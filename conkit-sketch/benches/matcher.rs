use conkit_sketch::{
    CatalogPath, CheckMode, CheckRequest, FileCatalog, ReportRequest, SketchContractKit,
    SketchContractKitBuilder, SketchLimits, SketchOccurrence, WorkOptions, WorkerPool,
};
use criterion::{BatchSize, Criterion, Throughput};
use std::fmt::Write as _;
use std::hint::black_box;
use std::num::NonZeroUsize;

struct SketchBenchmarks {
    kit: SketchBenchmarkKit,
}

impl SketchBenchmarks {
    fn register(criterion: &mut Criterion) {
        let benchmarks = Self {
            kit: SketchBenchmarkKit::new(),
        };
        let cardinality_cases = [
            SketchCase::matching_lines("ten-thousand-lines-4-sketches", 10_000, 4),
            SketchCase::matching_lines("ten-thousand-lines-64-sketches", 10_000, 64),
            SketchCase::matching_lines("ten-thousand-lines-256-sketches", 10_000, 256),
            SketchCase::matching_lines("ten-thousand-lines-1000-sketches", 10_000, 1_000),
        ];
        let byte_cases = [
            SketchCase::matching_lines("one-file-one-sketch", 100, 1),
            SketchCase::matching_lines("hundred-thousand-lines", 100_000, 64),
            SketchCase::many_files_few_sketches(64, 4),
            SketchCase::positioned("early-hit", MatchPosition::Early),
            SketchCase::positioned("late-hit", MatchPosition::Late),
            SketchCase::positioned("complete-miss", MatchPosition::Missing),
            SketchCase::positioned("common-prefix-miss", MatchPosition::CommonPrefixMissing),
            SketchCase::positioned("exactly-one-duplicate", MatchPosition::Duplicated),
            SketchCase::long_pattern("hundred-line-late-hit", 100, PatternOutcome::LateHit),
            SketchCase::long_pattern("hundred-line-late-miss", 100, PatternOutcome::LateMiss),
            SketchCase::long_pattern("thousand-line-late-hit", 1_000, PatternOutcome::LateHit),
            SketchCase::long_pattern("thousand-line-late-miss", 1_000, PatternOutcome::LateMiss),
            SketchCase::overlapping_occurrences(),
            SketchCase::invalid_source_bytes(),
        ];

        let mut cardinality_group =
            criterion.benchmark_group("grouped-linear-sketch-matching-by-sketch-count");
        for case in &cardinality_cases {
            cardinality_group.throughput(Throughput::Elements(
                u64::try_from(case.sketch_count).unwrap_or(u64::MAX),
            ));
            benchmarks.register_case(&mut cardinality_group, case);
        }
        cardinality_group.finish();

        let mut byte_group =
            criterion.benchmark_group("grouped-linear-sketch-matching-by-source-bytes");
        for case in &byte_cases {
            byte_group.throughput(Throughput::Bytes(case.source_bytes));
            benchmarks.register_case(&mut byte_group, case);
        }
        byte_group.finish();
    }

    fn register_case(
        &self,
        group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
        case: &SketchCase,
    ) {
        group.bench_with_input(case.name, case, |bencher, case| {
            bencher.iter_batched(
                || case.request(),
                |request| black_box(self.kit.check(request)),
                BatchSize::LargeInput,
            );
        });
    }
}

struct SketchBenchmarkKit {
    kit: SketchContractKit,
}

impl SketchBenchmarkKit {
    fn new() -> Self {
        let mut limits = SketchLimits::default();
        limits.matching.line_comparisons = u64::MAX;
        limits.matching.occurrence_candidates = u64::MAX;
        let kit = SketchContractKitBuilder::default()
            .with_work_options(WorkOptions {
                pool: WorkerPool::Dedicated {
                    worker_threads: NonZeroUsize::MIN,
                },
                max_in_flight_operations: NonZeroUsize::MIN,
                max_pending_operations: 0,
            })
            .with_limits(limits)
            .build()
            .expect("benchmark work pool must build");
        Self { kit }
    }

    fn check(&self, request: CheckRequest) -> conkit_sketch::CheckResponse {
        futures_executor::block_on(self.kit.check(request)).expect("benchmark check must complete")
    }
}

struct SketchCase {
    name: &'static str,
    source_files: FileCatalog,
    contract_files: FileCatalog,
    sketch_count: usize,
    source_bytes: u64,
}

impl SketchCase {
    fn matching_lines(name: &'static str, source_line_count: usize, sketch_count: usize) -> Self {
        let source = SourceLines::numbered(source_line_count);
        let positions = (0..sketch_count)
            .map(|index| {
                if sketch_count == 1 {
                    source_line_count.saturating_sub(1)
                } else {
                    index.saturating_mul(source_line_count.saturating_sub(1))
                        / sketch_count.saturating_sub(1)
                }
            })
            .collect::<Vec<_>>();
        let snippets = positions
            .into_iter()
            .map(|position| source.line(position))
            .collect::<Vec<_>>();

        Self::single_source(
            name,
            source.into_bytes(),
            snippets,
            SketchOccurrence::AtLeastOne,
        )
    }

    fn many_files_few_sketches(file_count: usize, sketch_count: usize) -> Self {
        let mut source_files = FileCatalog::new();
        let mut files = Vec::with_capacity(sketch_count);
        let mut snippets = Vec::with_capacity(sketch_count);
        for index in 0..file_count {
            let path = format!("file-{index:04}.rs");
            let line = format!("payload-{index:04}");
            source_files
                .insert(
                    CatalogPath::new(path.clone()).expect("generated source path"),
                    line.as_bytes().to_vec(),
                )
                .expect("generated paths are unique");
            if index < sketch_count {
                files.push(path);
                snippets.push(line);
            }
        }

        Self::with_sources(
            "many-files-few-sketches",
            source_files,
            files,
            snippets,
            SketchOccurrence::AtLeastOne,
        )
    }

    fn positioned(name: &'static str, position: MatchPosition) -> Self {
        match position {
            MatchPosition::Early => {
                let source = SourceLines::with_needle(10_000, 0, "target-line");
                Self::single_source(
                    name,
                    source,
                    vec!["target-line".to_owned()],
                    SketchOccurrence::AtLeastOne,
                )
            }
            MatchPosition::Late => {
                let source = SourceLines::with_needle(10_000, 9_999, "target-line");
                Self::single_source(
                    name,
                    source,
                    vec!["target-line".to_owned()],
                    SketchOccurrence::AtLeastOne,
                )
            }
            MatchPosition::Missing => Self::single_source(
                name,
                SourceLines::numbered(10_000).into_bytes(),
                vec!["never-present".to_owned()],
                SketchOccurrence::AtLeastOne,
            ),
            MatchPosition::CommonPrefixMissing => {
                let mut source = String::new();
                for _ in 0..2_500 {
                    source.push_str("common-a\ncommon-b\ncommon-c\nactual-tail\n");
                }
                Self::single_source(
                    name,
                    source.into_bytes(),
                    vec!["common-a\ncommon-b\ncommon-c\nexpected-tail".to_owned()],
                    SketchOccurrence::AtLeastOne,
                )
            }
            MatchPosition::Duplicated => Self::single_source(
                name,
                b"duplicate\nfiller\nduplicate\n".to_vec(),
                vec!["duplicate".to_owned()],
                SketchOccurrence::ExactlyOne,
            ),
        }
    }

    fn invalid_source_bytes() -> Self {
        Self::single_source(
            "invalid-source-bytes",
            b"prefix\n\xff\xfe\nneedle\n".to_vec(),
            vec!["needle".to_owned()],
            SketchOccurrence::AtLeastOne,
        )
    }

    fn long_pattern(
        name: &'static str,
        pattern_line_count: usize,
        outcome: PatternOutcome,
    ) -> Self {
        let source_line_count = 10_000;
        let source = SourceLines::numbered(source_line_count);
        let start = source_line_count.saturating_sub(pattern_line_count);
        let mut snippet = source.snippet(start, pattern_line_count);
        if outcome == PatternOutcome::LateMiss {
            snippet.push_str("-missing-tail");
        }
        Self::single_source(
            name,
            source.into_bytes(),
            vec![snippet],
            SketchOccurrence::AtLeastOne,
        )
    }

    fn overlapping_occurrences() -> Self {
        Self::single_source(
            "exactly-one-overlapping-occurrences",
            b"overlap\noverlap\noverlap\n".to_vec(),
            vec!["overlap\noverlap".to_owned()],
            SketchOccurrence::ExactlyOne,
        )
    }

    fn single_source(
        name: &'static str,
        source: Vec<u8>,
        snippets: Vec<String>,
        occurrence: SketchOccurrence,
    ) -> Self {
        let mut source_files = FileCatalog::new();
        source_files
            .insert(
                CatalogPath::new("shared.rs").expect("static source path"),
                source,
            )
            .expect("single source insert");
        let files = vec!["shared.rs".to_owned(); snippets.len()];
        Self::with_sources(name, source_files, files, snippets, occurrence)
    }

    fn with_sources(
        name: &'static str,
        source_files: FileCatalog,
        files: Vec<String>,
        snippets: Vec<String>,
        occurrence: SketchOccurrence,
    ) -> Self {
        assert_eq!(files.len(), snippets.len());
        let sketch_count = snippets.len();
        let source_bytes = source_files
            .iter()
            .map(|(_, bytes)| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .fold(0_u64, u64::saturating_add);
        let contract = SketchContractFixture::new(files, snippets, occurrence).render();
        let mut contract_files = FileCatalog::new();
        contract_files
            .insert(
                CatalogPath::new("main.yml").expect("static contract path"),
                contract.into_bytes(),
            )
            .expect("single contract insert");
        Self {
            name,
            source_files,
            contract_files,
            sketch_count,
            source_bytes,
        }
    }

    fn request(&self) -> CheckRequest {
        CheckRequest {
            source_files: self.source_files.clone(),
            contract_files: self.contract_files.clone(),
            report: ReportRequest::None,
            mode: CheckMode::Warning,
        }
    }
}

enum MatchPosition {
    Early,
    Late,
    Missing,
    CommonPrefixMissing,
    Duplicated,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum PatternOutcome {
    LateHit,
    LateMiss,
}

struct SourceLines {
    bytes: Vec<u8>,
    offsets: Vec<(usize, usize)>,
}

impl SourceLines {
    fn numbered(line_count: usize) -> Self {
        let mut bytes = Vec::with_capacity(line_count.saturating_mul(16));
        let mut offsets = Vec::with_capacity(line_count);
        for index in 0..line_count {
            let start = bytes.len();
            let line = format!("line-{index:08}\n");
            bytes.extend_from_slice(line.as_bytes());
            offsets.push((start, bytes.len() - 1));
        }
        Self { bytes, offsets }
    }

    fn with_needle(line_count: usize, needle_index: usize, needle: &str) -> Vec<u8> {
        let mut source = Self::numbered(line_count);
        let (start, end) = source.offsets[needle_index];
        let mut replacement = needle.as_bytes().to_vec();
        replacement.push(b'\n');
        source.bytes.splice(start..=end, replacement);
        source.bytes
    }

    fn line(&self, index: usize) -> String {
        let (start, end) = self.offsets[index];
        String::from_utf8(self.bytes[start..end].to_vec()).expect("numbered line is UTF-8")
    }

    fn snippet(&self, start: usize, line_count: usize) -> String {
        (start..start.saturating_add(line_count))
            .map(|index| self.line(index))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

struct SketchContractFixture {
    files: Vec<String>,
    snippets: Vec<String>,
    occurrence: SketchOccurrence,
}

impl SketchContractFixture {
    fn new(files: Vec<String>, snippets: Vec<String>, occurrence: SketchOccurrence) -> Self {
        Self {
            files,
            snippets,
            occurrence,
        }
    }

    fn occurrence_name(&self) -> &'static str {
        match self.occurrence {
            SketchOccurrence::AtLeastOne => "at_least_one",
            SketchOccurrence::ExactlyOne => "exactly_one",
        }
    }

    fn render(&self) -> String {
        let crate_root = self
            .files
            .first()
            .expect("a benchmark contract has at least one source file");
        let mut yaml = String::from("contract_version: 2\nroot: ../src\nfiles:\n");
        for file in self.files.iter().collect::<std::collections::BTreeSet<_>>() {
            writeln!(yaml, "  - {file}").expect("writing to String cannot fail");
        }
        write!(
            yaml,
            "extraction:\n  mode: rust_syntax_v2\n  profile: rust_api_v1\n  crates:\n    - id: benchmark\n      root: {crate_root}\n      kind: library\nsignatures:\n",
        )
        .expect("writing to String cannot fail");
        for (index, file) in self.files.iter().enumerate() {
            write!(
                yaml,
                "  - signature-{index}:\n      file: {file}\n      signature_type: function\n      sketch: sketch-{index}\n",
            )
            .expect("writing to String cannot fail");
        }
        yaml.push_str("sketches:\n");
        for (index, (file, snippet)) in self.files.iter().zip(&self.snippets).enumerate() {
            write!(
                yaml,
                "  - sketch-{index}:\n      file: {file}\n      signature: signature-{index}\n      signature_type: function\n      matching:\n        normalization: exact_lines_v1\n        occurrence: {}\n      code: |-\n",
                self.occurrence_name(),
            )
            .expect("writing to String cannot fail");
            for line in snippet.split('\n') {
                writeln!(yaml, "        {line}").expect("writing to String cannot fail");
            }
        }
        yaml
    }
}

criterion::criterion_group!(matcher_benches, SketchBenchmarks::register);
criterion::criterion_main!(matcher_benches);
