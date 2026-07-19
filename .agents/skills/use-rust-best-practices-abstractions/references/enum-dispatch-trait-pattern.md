# Enum-dispatch trait pattern

Use this reference before adding, deleting, or refactoring enum dispatch over a
closed implementation family. First establish that the family has multiple
real concrete implementations; this pattern is not a default wrapper for a
single behavior owner.

## Contents

- [Review source-backed findings](#1-source-backed-findings)
- [Choose the dispatch shape](#2-decision-checklist)
- [Apply the required shape](#3-required-shape)
- [Follow the synchronous example](#4-sync-example)
- [Follow the native async example](#5-native-async-example)
- [Reject anti-patterns](#6-anti-patterns)
- [Complete the review checklist](#7-review-checklist)

## 1. Source-backed findings

- The Rust Book presents enums as values that are one of a fixed set of
  variants, and variants can carry data directly:
  <https://doc.rust-lang.org/book/ch06-01-defining-an-enum.html>.
- The Rust Book presents `match` as the normal way to bind enum variant data,
  and `match` arms must cover every possibility:
  <https://doc.rust-lang.org/book/ch06-02-match.html>.
- Rust pattern syntax includes `_`, named bindings, `|`, and ranges, but
  implementation-family dispatch should use explicit variant arms instead of a
  wildcard arm so new variants force a deliberate update:
  <https://doc.rust-lang.org/book/ch19-03-pattern-syntax.html>.
- The Rust Reference defines traits as abstract interfaces, and trait impls
  must provide all non-default associated items and cannot add extra ones:
  <https://doc.rust-lang.org/reference/items/traits.html> and
  <https://doc.rust-lang.org/reference/items/implementations.html>.
- Native `async fn` in traits is appropriate for private/internal traits when
  old MSRV support and dynamic dispatch are not needed:
  <https://blog.rust-lang.org/2023/12/21/async-fn-rpit-in-traits/>.
- Traits with `async fn` are not dyn-compatible because they return an opaque
  future, which is acceptable for closed enum dispatch that does not use
  `dyn Trait`:
  <https://doc.rust-lang.org/reference/items/traits.html#dyn-compatibility>.
- Rust API Guidelines support methods on clear receivers, builders for complex
  construction or choices, private/sealed traits for future flexibility, and
  caution around macros:
  <https://rust-lang.github.io/api-guidelines/predictability.html>,
  <https://rust-lang.github.io/api-guidelines/type-safety.html>,
  <https://rust-lang.github.io/api-guidelines/future-proofing.html>, and
  <https://doc.rust-lang.org/book/ch20-05-macros.html>.

## 2. Decision checklist

Use a data-only enum when the enum only names states or options and does not
dispatch implementation behavior.

Use generics when the caller chooses one implementation statically and the API
should be monomorphized.

Use a trait object when the implementation set is open at runtime, heterogeneous
values must be stored behind one handle, and dynamic dispatch is an intentional
part of the design.

Use enum dispatch plus a private trait only when multiple concrete
implementations already form a closed family inside the crate and callers
benefit from one exhaustive routing surface. Exported root dispatchers can use
opaque public structs over private inner enums so callers cannot name
variants. This opacity wrapper is only for public exports; crate-private
internal dispatch enums do not need struct wrappers. These private enums are
still dispatch machinery, not a license to create reusable family identity,
provenance, capability, diagnostic-label, or error-label utilities.

In Contract Kit, `RustExtractionBackend` is the current closed family: syntax
and compiler extraction implement the same private contract behind
`RustBackend`. `SketchContractKit` remains a direct concrete owner and should
not acquire this pattern unless a second real implementation appears.

## 3. Required shape

Apply the following shape only after the decision checklist establishes a real
closed multi-implementation family:

1. Define one implementation-agnostic private or `pub(crate)` trait for the
   behavior family.
2. For exported dispatchers, expose a public struct with a private inner enum.
   For crate-private dispatchers, a plain private enum is enough.
3. Make each private enum variant wrap a concrete implementation struct.
4. Implement the trait for every concrete struct in that struct's owning module
   subtree.
5. Implement the trait for the public handle or private dispatcher enum in the
   dispatcher module using explicit, exhaustive `match` arms whose arms call
   receiver methods on payloads that implement the same trait. In Contract
   Kit, the current domain example is `RustBackend` routing syntax/compiler
   extraction in its owning backend module.
6. Do not keep same-name inherent forwarding methods as the operation parity
   surface; the private trait impl on the receiver is the parity surface.
7. Keep shared contract modules limited to implementation-agnostic traits, shared
   handles, and default helpers.
8. Keep public builders/configs responsible for choosing and validating the
   concrete enum variant.
9. Cover the contract with focused tests that require the trait definition,
   appropriate public opacity when exported, concrete impl placement,
   receiver-method dispatch, and the absence of macro-generated replacements.
10. Keep implementation-family facts inside the owning payload or documented
    data/config type. Do not introduce shared `*ParserKind`, `*RunnerKind`,
    provenance fields, display-name helpers, capability-label renderers, or
    implementation-family-prefixed diagnostics as shortcuts around the
    dispatcher boundary.

## 4. Sync example

```rust
pub(crate) trait SearchBackend {
    fn search(&self, req: SearchRequest) -> Result<SearchResponse, SearchError>;
}

pub struct SearchEngine {
    inner: SearchEngineInner,
}

enum SearchEngineInner {
    Local(local::LocalSearch),
    Remote(remote::RemoteSearch),
}

impl SearchBackend for SearchEngine {
    fn search(&self, req: SearchRequest) -> Result<SearchResponse, SearchError> {
        match &self.inner {
            SearchEngineInner::Local(inner) => inner.search(req),
            SearchEngineInner::Remote(inner) => inner.search(req),
        }
    }
}

mod local {
    use super::{SearchBackend, SearchError, SearchRequest, SearchResponse};

    pub struct LocalSearch;

    impl SearchBackend for LocalSearch {
        fn search(&self, _req: SearchRequest) -> Result<SearchResponse, SearchError> {
            Ok(SearchResponse)
        }
    }
}

mod remote {
    use super::{SearchBackend, SearchError, SearchRequest, SearchResponse};

    pub struct RemoteSearch;

    impl SearchBackend for RemoteSearch {
        fn search(&self, _req: SearchRequest) -> Result<SearchResponse, SearchError> {
            Ok(SearchResponse)
        }
    }
}

pub struct SearchRequest;
pub struct SearchResponse;

#[derive(Debug, thiserror::Error)]
#[error("search failed")]
pub struct SearchError;
```

In a real module tree, `impl SearchBackend for LocalSearch` belongs under the
`local` subtree, and `impl SearchBackend for RemoteSearch` belongs under the
`remote` subtree. The shared contract module must not collect concrete impls.

## 5. Native async example

```rust
pub(crate) trait SearchBackend {
    async fn search(&self, req: SearchRequest) -> Result<SearchResponse, SearchError>;
}

pub struct SearchEngine {
    inner: SearchEngineInner,
}

enum SearchEngineInner {
    Local(local::LocalSearch),
    Remote(remote::RemoteSearch),
}

impl SearchBackend for SearchEngine {
    async fn search(&self, req: SearchRequest) -> Result<SearchResponse, SearchError> {
        match &self.inner {
            SearchEngineInner::Local(inner) => inner.search(req).await,
            SearchEngineInner::Remote(inner) => inner.search(req).await,
        }
    }
}

mod local {
    use super::{SearchBackend, SearchError, SearchRequest, SearchResponse};

    pub struct LocalSearch;

    impl SearchBackend for LocalSearch {
        async fn search(&self, _req: SearchRequest) -> Result<SearchResponse, SearchError> {
            Ok(SearchResponse)
        }
    }
}

mod remote {
    use super::{SearchBackend, SearchError, SearchRequest, SearchResponse};

    pub struct RemoteSearch;

    impl SearchBackend for RemoteSearch {
        async fn search(&self, _req: SearchRequest) -> Result<SearchResponse, SearchError> {
            Ok(SearchResponse)
        }
    }
}

pub struct SearchRequest;
pub struct SearchResponse;

#[derive(Debug, thiserror::Error)]
#[error("search failed")]
pub struct SearchError;
```

Do not add `async_trait` for this pattern. If a design truly requires dynamic
dispatch over async trait methods, that is no longer the closed enum-dispatch
pattern and needs a separate design decision.

## 6. Anti-patterns

Do not rely on inherent-method parity by convention:

```rust
impl SearchEngine {
    pub async fn search(&self, req: SearchRequest) -> Result<SearchResponse, SearchError> {
        match &self.inner {
            SearchEngineInner::Local(inner) => inner.search(req).await,
            SearchEngineInner::Remote(inner) => inner.search(req).await,
        }
    }
}
```

Do not write the same dispatcher as
`SearchBackend::search(inner, req)`. UFCS is useful for rare disambiguation,
but in this workspace it is invalid for enum-dispatch contract calls because it
usually signals the receiver does not own the operation cleanly or the code has
same-name inherent forwarding methods that bypass trait parity.

The one valid UFCS-shaped call in this pattern is a public root handle method that
bridges into the opaque handle's own private trait impl to avoid recursive
same-name inherent calls:

```rust
impl SearchEngine {
    pub async fn search(&self, req: SearchRequest) -> Result<SearchResponse, SearchError> {
        <Self as SearchBackend>::search(self, req).await
    }
}
```

That bridge does not dispatch to a payload. The actual dispatch remains inside
`impl SearchBackend for SearchEngine`, where each arm calls receiver methods on
trait-implementing payload structs.

This compiles even if a concrete implementation misses a method. The private
trait is the parity contract.

Do not use wildcard arms for implementation-family dispatch:

```rust
match self {
    Self::Local(inner) => inner.search(req).await,
    _ => Err(SearchError),
}
```

The wildcard hides new variants from the compiler's exhaustiveness pressure.

Do not move concrete impls into the shared contract module:

```rust
// contract.rs
impl ContractParser for yaml::YamlContractParser {
    /* ... */
}
```

Concrete implementation knowledge belongs with the concrete implementation.

Do not replace hand-written dispatch with `macro_rules!`, proc macros,
generated dispatch, generated tables, public facade traits, compatibility
shims, or `async_trait`.

Do not use a private dispatcher or config-selection enum as a reusable identity
object:

```rust
pub(crate) enum ParserKind {
    Yaml,
    Json,
}

impl ParserKind {
    pub(crate) const fn display_name(self) -> &'static str {
        match self {
            Self::Yaml => "YAML",
            Self::Json => "JSON",
        }
    }
}
```

That pattern leaks implementation-family identity into callers that should be
talking to the owning payload or the private dispatch trait.

## 7. Review checklist

- Does the family have at least two real implementations, or should it remain
  concrete?
- For a justified behavior-dispatch enum, does one unique private trait define
  the contract?
- Does every enum variant wrap a concrete struct rather than loose family
  data?
- Does the enum trait impl use explicit match arms for every variant?
- Are concrete trait impls in the owning implementation subtree?
- Are shared contract modules implementation-agnostic?
- Are public builders/configs still the construction boundary?
- Are implementation-family identity, provenance, capability, and diagnostics
  kept out of shared kind/helper enums?
- Are wildcard/catch-all dispatch arms and stale restoration TODO markers
  absent for implementation families?
- Do tests fail if the trait, enum impl, concrete impl placement, or explicit
  match dispatch disappears?
- In Contract Kit, does `RustExtractionBackend` remain the syntax/compiler
  dispatcher while `SketchContractKit` remains direct?
