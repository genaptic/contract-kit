use conkit_signature::{
    CatalogPath, CheckMode, CheckRequest, CheckResponse, ContractScope, FileCatalog,
    GenerateDocument, GenerateRequest, GenerateResponse, GenerateTarget, ReportRequest,
    ResolveSketchesRequest, ResolveSketchesResponse, RustCrateKind, RustCrateRoot,
    RustExtractionInput, SignatureContractKit, SignatureContractKitBuilder, WorkOptions,
    WorkerPool,
};
use criterion::{BatchSize, BenchmarkId, Criterion, Throughput};
use std::fmt::Write as _;
use std::hint::black_box;
use std::num::NonZeroUsize;

struct ParsingBenchmarks {
    cases: Vec<ParsingCase>,
}

impl ParsingBenchmarks {
    fn register(criterion: &mut Criterion) {
        let setup_kit = BenchmarkKit::new(1);
        let suite = Self::new(&setup_kit);
        let mut group = criterion.benchmark_group("allowlist-bounded-signature-parsing");

        for case in &suite.cases {
            group.throughput(Throughput::Bytes(case.catalog_bytes));
            for worker_threads in [1, 4] {
                let kit = BenchmarkKit::new(worker_threads);
                let worker_label = format!("{worker_threads}-workers");

                group.bench_with_input(
                    BenchmarkId::new(format!("{}/check", case.name), worker_label.as_str()),
                    case,
                    |bencher, case| {
                        bencher.iter_batched(
                            || case.check_request(),
                            |request| black_box(kit.check(request)),
                            BatchSize::LargeInput,
                        );
                    },
                );
                group.bench_with_input(
                    BenchmarkId::new(format!("{}/generate-new", case.name), worker_label.as_str()),
                    case,
                    |bencher, case| {
                        bencher.iter_batched(
                            || case.new_generation_request(),
                            |request| black_box(kit.generate(request)),
                            BatchSize::LargeInput,
                        );
                    },
                );
                group.bench_with_input(
                    BenchmarkId::new(
                        format!("{}/generate-existing", case.name),
                        worker_label.as_str(),
                    ),
                    case,
                    |bencher, case| {
                        bencher.iter_batched(
                            || case.existing_generation_request(),
                            |request| black_box(kit.generate(request)),
                            BatchSize::LargeInput,
                        );
                    },
                );
                group.bench_with_input(
                    BenchmarkId::new(
                        format!("{}/resolve-sketches", case.name),
                        worker_label.as_str(),
                    ),
                    case,
                    |bencher, case| {
                        bencher.iter_batched(
                            || case.resolve_request(),
                            |request| black_box(kit.resolve(request)),
                            BatchSize::LargeInput,
                        );
                    },
                );
            }
        }

        group.finish();
    }

    fn new(setup_kit: &BenchmarkKit) -> Self {
        let corpora = [
            SourceCorpus::uniform("one-1kib-file", 1, 1_024),
            SourceCorpus::uniform("eight-small-files", 8, 1_024),
            SourceCorpus::uniform("sixty-four-small-files", 64, 1_024),
            SourceCorpus::uniform("eight-medium-files", 8, 64 * 1_024),
            SourceCorpus::skewed("one-large-many-small", 1_024 * 1_024, 64, 1_024),
            SourceCorpus::mostly_unlisted("many-unlisted-tiny-allowlist", 512, 1_024),
            SourceCorpus::semantic("item-attribute-impl-heavy", 512 * 1_024),
        ];
        let cases = corpora
            .into_iter()
            .map(|corpus| ParsingCase::new(corpus, setup_kit))
            .collect();
        Self { cases }
    }
}

struct BenchmarkKit {
    kit: SignatureContractKit,
}

impl BenchmarkKit {
    fn new(worker_threads: usize) -> Self {
        let worker_threads =
            NonZeroUsize::new(worker_threads).expect("benchmark worker count is nonzero");
        let kit = SignatureContractKitBuilder::default()
            .with_work_options(WorkOptions {
                pool: WorkerPool::Dedicated { worker_threads },
                max_in_flight_operations: NonZeroUsize::MIN,
                max_pending_operations: 0,
            })
            .build()
            .expect("benchmark work pool must build");
        Self { kit }
    }

    fn check(&self, request: CheckRequest) -> CheckResponse {
        futures_executor::block_on(self.kit.check(request)).expect("benchmark check must complete")
    }

    fn generate(&self, request: GenerateRequest) -> GenerateResponse {
        futures_executor::block_on(self.kit.generate(request))
            .expect("benchmark generation must complete")
    }

    fn resolve(&self, request: ResolveSketchesRequest) -> ResolveSketchesResponse {
        futures_executor::block_on(self.kit.resolve_sketches(request))
            .expect("benchmark sketch resolution must complete")
    }
}

struct ParsingCase {
    name: &'static str,
    source_files: FileCatalog,
    document: GenerateDocument,
    generated_contract_files: FileCatalog,
    linked_contract_files: FileCatalog,
    catalog_bytes: u64,
}

impl ParsingCase {
    fn new(corpus: SourceCorpus, setup_kit: &BenchmarkKit) -> Self {
        let generated_contract_files = setup_kit
            .generate(GenerateRequest {
                source_files: corpus.source_files.clone(),
                target: GenerateTarget::New(corpus.document.clone()),
                extraction: RustExtractionInput::Syntax,
                scope: ContractScope::Signatures,
            })
            .contract_files;
        let linked_contract_files = corpus.linked_contract_files();
        Self {
            name: corpus.name,
            source_files: corpus.source_files,
            document: corpus.document,
            generated_contract_files,
            linked_contract_files,
            catalog_bytes: corpus.catalog_bytes,
        }
    }

    fn check_request(&self) -> CheckRequest {
        CheckRequest {
            source_files: self.source_files.clone(),
            contract_files: self.generated_contract_files.clone(),
            extraction: RustExtractionInput::Syntax,
            report: ReportRequest::None,
            mode: CheckMode::Default,
        }
    }

    fn new_generation_request(&self) -> GenerateRequest {
        GenerateRequest {
            source_files: self.source_files.clone(),
            target: GenerateTarget::New(self.document.clone()),
            extraction: RustExtractionInput::Syntax,
            scope: ContractScope::Signatures,
        }
    }

    fn existing_generation_request(&self) -> GenerateRequest {
        GenerateRequest {
            source_files: self.source_files.clone(),
            target: GenerateTarget::Existing(self.generated_contract_files.clone()),
            extraction: RustExtractionInput::Syntax,
            scope: ContractScope::Signatures,
        }
    }

    fn resolve_request(&self) -> ResolveSketchesRequest {
        ResolveSketchesRequest {
            source_files: self.source_files.clone(),
            contract_files: self.linked_contract_files.clone(),
            extraction: RustExtractionInput::Syntax,
        }
    }
}

struct SourceCorpus {
    name: &'static str,
    source_files: FileCatalog,
    document: GenerateDocument,
    catalog_bytes: u64,
}

impl SourceCorpus {
    fn uniform(name: &'static str, file_count: usize, bytes_per_file: usize) -> Self {
        Self::build(
            name,
            vec![bytes_per_file; file_count],
            0,
            0,
            CorpusContent::InertPadding,
        )
    }

    fn skewed(
        name: &'static str,
        large_file_bytes: usize,
        small_file_count: usize,
        small_file_bytes: usize,
    ) -> Self {
        let mut sizes = Vec::with_capacity(small_file_count.saturating_add(1));
        sizes.push(large_file_bytes);
        sizes.extend(std::iter::repeat_n(small_file_bytes, small_file_count));
        Self::build(name, sizes, 0, 0, CorpusContent::InertPadding)
    }

    fn mostly_unlisted(
        name: &'static str,
        unlisted_file_count: usize,
        unlisted_file_bytes: usize,
    ) -> Self {
        Self::build(
            name,
            vec![1_024],
            unlisted_file_count,
            unlisted_file_bytes,
            CorpusContent::InertPadding,
        )
    }

    fn semantic(name: &'static str, source_bytes: usize) -> Self {
        Self::build(name, vec![source_bytes], 0, 0, CorpusContent::SemanticItems)
    }

    fn build(
        name: &'static str,
        participating_sizes: Vec<usize>,
        unlisted_file_count: usize,
        unlisted_file_bytes: usize,
        content: CorpusContent,
    ) -> Self {
        assert!(!participating_sizes.is_empty());
        let participating_file_count = participating_sizes.len();
        let mut source_files = FileCatalog::new();
        let mut files = Vec::with_capacity(participating_file_count);

        for (index, target_bytes) in participating_sizes.into_iter().enumerate() {
            let path = if index == 0 {
                "lib.rs".to_owned()
            } else {
                format!("u{index}.rs")
            };
            let bytes = if index == 0 {
                Self::root_source(target_bytes, participating_file_count, content)
            } else {
                Self::module_source(index, target_bytes, content)
            };
            let path = CatalogPath::new(path).expect("generated participating source path");
            source_files
                .insert(path.clone(), bytes)
                .expect("generated participating paths are unique");
            files.push(path);
        }

        for index in 0..unlisted_file_count {
            let path = CatalogPath::new(format!("ignored_{index:04}.rs"))
                .expect("generated unlisted source path");
            source_files
                .insert(path, vec![0xff; unlisted_file_bytes])
                .expect("generated unlisted paths are unique");
        }

        let catalog_bytes = source_files
            .iter()
            .map(|(_, bytes)| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
            .fold(0_u64, u64::saturating_add);
        let document = GenerateDocument {
            contract_file: CatalogPath::new("main.yml").expect("static contract path"),
            root: "../src".to_owned(),
            files,
            crates: vec![RustCrateRoot {
                id: "benchmark".to_owned(),
                root: CatalogPath::new("lib.rs").expect("static crate root path"),
                kind: RustCrateKind::Library,
            }],
        };
        Self {
            name,
            source_files,
            document,
            catalog_bytes,
        }
    }

    fn root_source(target_bytes: usize, file_count: usize, content: CorpusContent) -> Vec<u8> {
        let mut source = match content {
            CorpusContent::InertPadding => String::from("pub fn benchmark_anchor() {}\n"),
            CorpusContent::SemanticItems => String::from(
                "#[repr(C)]\npub struct BenchmarkOwner {\n    pub value: usize,\n}\n\nimpl BenchmarkOwner {\n    #[must_use]\n    pub const fn new(value: usize) -> Self {\n        Self { value }\n    }\n}\n\n#[non_exhaustive]\npub enum BenchmarkState {\n    Ready,\n    Waiting { attempts: usize },\n}\n\npub trait BenchmarkTrait {\n    type Output;\n    const ENABLED: bool;\n    fn output(&self) -> Self::Output;\n}\n\npub fn benchmark_anchor() {}\n",
            ),
        };
        for index in 1..file_count {
            writeln!(source, "pub mod u{index};").expect("writing to String cannot fail");
        }
        Self::padded_source(source, target_bytes, content)
    }

    fn module_source(index: usize, target_bytes: usize, content: CorpusContent) -> Vec<u8> {
        Self::padded_source(
            format!("pub fn benchmark_item_{index:04}() {{}}\n"),
            target_bytes,
            content,
        )
    }

    fn padded_source(mut source: String, target_bytes: usize, content: CorpusContent) -> Vec<u8> {
        const PADDING_LINE: &str = "// benchmark parser padding bytes stay syntactically inert\n";

        match content {
            CorpusContent::InertPadding => {
                while source.len().saturating_add(PADDING_LINE.len()) <= target_bytes {
                    source.push_str(PADDING_LINE);
                }
            }
            CorpusContent::SemanticItems => {
                for index in 0_usize.. {
                    let item = format!(
                        "#[deprecated(note = \"benchmark corpus\")]\npub const BENCHMARK_SEMANTIC_ITEM_{index:06}: usize = {index};\n"
                    );
                    if source.len().saturating_add(item.len()) > target_bytes {
                        break;
                    }
                    source.push_str(&item);
                }
            }
        }
        if source.len() < target_bytes {
            let remaining = target_bytes - source.len();
            match remaining {
                1 => source.push('\n'),
                2 => source.push_str("//"),
                _ => {
                    source.push_str("//");
                    source.push_str(&"x".repeat(remaining - 2));
                }
            }
        }
        source.into_bytes()
    }

    fn linked_contract_files(&self) -> FileCatalog {
        let mut yaml = String::from("contract_version: 2\nroot: ../src\nfiles:\n");
        for path in &self.document.files {
            writeln!(yaml, "  - {path}").expect("writing to String cannot fail");
        }
        yaml.push_str(
            "extraction:\n  mode: rust_syntax_v2\n  profile: rust_api_v1\n  crates:\n    - id: benchmark\n      root: lib.rs\n      kind: library\nsignatures:\n  - benchmark_anchor_function:\n      file: lib.rs\n      signature_type: function\n      name: benchmark_anchor\n      visibility: public\n      sketch: benchmark-anchor\nsketches:\n  - benchmark-anchor:\n      file: lib.rs\n      signature: benchmark_anchor_function\n      signature_type: function\n      matching:\n        normalization: exact_lines_v1\n        occurrence: exactly_one\n      code: |-\n        pub fn benchmark_anchor() {}\n",
        );
        let mut contracts = FileCatalog::new();
        contracts
            .insert(self.document.contract_file.clone(), yaml.into_bytes())
            .expect("single linked contract insert");
        contracts
    }
}

#[derive(Clone, Copy)]
enum CorpusContent {
    InertPadding,
    SemanticItems,
}

criterion::criterion_group!(source_parsing_benches, ParsingBenchmarks::register);
criterion::criterion_main!(source_parsing_benches);
