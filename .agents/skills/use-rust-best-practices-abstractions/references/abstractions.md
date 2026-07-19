# Rust abstractions, lifetimes, errors, and panics

Use this reference when refactoring repeated logic, designing APIs, choosing abstraction mechanisms, or clarifying panic and unsafe policy.

## Contents

- [Keep code concrete until pressure is real](#1-keep-code-concrete-until-pressure-is-real)
- [Prefer structs with methods](#2-prefer-structs-with-methods-over-parallel-free-functions)
- [Choose the right abstraction](#3-choose-the-right-abstraction-mechanism)
- [Use enum dispatch for proven closed families](#4-use-enum-dispatch-only-for-proven-closed-configurable-families)
- [Use lifetimes when borrow relationships matter](#5-use-lifetimes-only-when-the-borrow-relationship-matters)
- [Prefer typed library errors](#6-prefer-typed-errors-in-libraries)
- [Reserve panics for bugs and impossible invariants](#7-panic-only-for-bugs-or-impossible-invariants)
- [Keep unsafe blocks narrow](#8-keep-unsafe-blocks-tiny-and-behind-safe-abstractions)
- [Review abstraction practices](#9-abstraction-dos-and-donts)
- [Further reading](#further-reading)
- [Read additional examples](#10-additional-merged-examples)
- [Additional source links](#additional-source-links)

## 1. Keep code concrete until pressure is real

Start concrete when there is only one implementation and one obvious call path.

### Good first step

```rust
pub struct PriceCalculator {
    tax_rate: f64,
}

impl PriceCalculator {
    pub fn new(tax_rate: f64) -> Self {
        Self { tax_rate }
    }

    pub fn total(&self, subtotal: f64) -> f64 {
        subtotal * (1.0 + self.tax_rate)
    }
}
```

Do not introduce a trait, generic parameter, or enum-dispatch wrapper just because the code might grow later.

Refactor when you see one of these signals:

- repeated business rules across call sites
- multiple real implementations
- repeated argument groups that should become a struct
- repeated branching over a closed set of variants
- repeated need for test doubles at a boundary

## 2. Prefer structs with methods over parallel free functions

### Bad

```rust
pub fn create_invoice(customer_id: &str, due_days: u32, currency: &str) -> Invoice {
    Invoice {
        customer_id: customer_id.to_owned(),
        due_days,
        currency: currency.to_owned(),
    }
}

pub fn validate_invoice(customer_id: &str, due_days: u32, currency: &str) -> Result<(), String> {
    if customer_id.is_empty() {
        return Err("customer id required".to_owned());
    }

    if due_days == 0 {
        return Err("due days must be positive".to_owned());
    }

    if currency.len() != 3 {
        return Err("currency must be ISO-4217".to_owned());
    }

    Ok(())
}

#[derive(Debug)]
pub struct Invoice {
    customer_id: String,
    due_days: u32,
    currency: String,
}
```

### Better

```rust
#[derive(Debug, Clone)]
pub struct InvoiceDraft {
    customer_id: String,
    due_days: u32,
    currency: String,
}

impl InvoiceDraft {
    pub fn new(customer_id: impl Into<String>, due_days: u32, currency: impl Into<String>) -> Self {
        Self {
            customer_id: customer_id.into(),
            due_days,
            currency: currency.into(),
        }
    }

    pub fn validate(&self) -> Result<(), InvoiceError> {
        if self.customer_id.is_empty() {
            return Err(InvoiceError::MissingCustomerId);
        }

        if self.due_days == 0 {
            return Err(InvoiceError::InvalidDueDays);
        }

        if self.currency.len() != 3 {
            return Err(InvoiceError::InvalidCurrency(self.currency.clone()));
        }

        Ok(())
    }

    pub fn build(self) -> Result<Invoice, InvoiceError> {
        self.validate()?;
        Ok(Invoice {
            customer_id: self.customer_id,
            due_days: self.due_days,
            currency: self.currency,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Invoice {
    customer_id: String,
    due_days: u32,
    currency: String,
}

#[derive(Debug, thiserror::Error)]
pub enum InvoiceError {
    #[error("customer id is required")]
    MissingCustomerId,
    #[error("due days must be positive")]
    InvalidDueDays,
    #[error("invalid currency: {0}")]
    InvalidCurrency(String),
}
```

This groups related data and behavior coherently.

## 3. Choose the right abstraction mechanism

### Stay concrete when

- there is one implementation
- there is no real extension point
- indirection would only make the code harder to read

### Use generics when

- the caller chooses the implementation at compile time
- performance and inlining matter
- the abstraction is broadly useful and type-driven

```rust
pub fn sum_prices<I>(items: I) -> u64
where
    I: IntoIterator<Item = u64>,
{
    items.into_iter().sum()
}
```

### Use a trait when

- multiple implementations are real today
- callers benefit from a shared interface
- tests need boundary-level fakes

```rust
pub(crate) trait UserRepository {
    async fn fetch(&self, user_id: u64) -> Result<User, RepoError>;
}

#[derive(Debug, Clone)]
pub struct User {
    pub id: u64,
}

#[derive(Debug, thiserror::Error)]
#[error("repository failure")]
pub struct RepoError;
```

### Use a trait object when

- the set of implementations is open at runtime
- dynamic dispatch is acceptable
- you need heterogeneous values behind one interface

```rust
pub struct Processor {
    backends: Vec<Box<dyn JobBackend>>,
}

pub trait JobBackend {
    fn name(&self) -> &str;
    fn run(&self);
}
```

### Use an enum when

- the set of variants is closed
- pattern matching is valuable
- you want exhaustiveness and direct control

```rust
pub enum OutputFormat {
    Json,
    Yaml,
    Text,
}
```

## 4. Use enum dispatch only for proven closed configurable families

Do not start with enum dispatch. Use it when all of these conditions are true:

- a trait representing behavior
- at least two real concrete implementations
- a deliberately closed set of implementation-family members
- a config enum that chooses the implementation
- a stable usage boundary that benefits from exhaustive routing

An exported family may use a builder that validates config and returns an
opaque public handle over a private inner enum. A crate-private family can use
a private enum directly. A single concrete behavior owner should stay concrete
until a second implementation proves that the routing layer is needed.

Read `enum-dispatch-trait-pattern.md` before changing this pattern.

In Contract Kit, the private `RustExtractionBackend` trait and `RustBackend`
enum form the current closed family because syntax and compiler extraction are
two real implementations of the same operations. Each concrete backend owns
its impl, and `RustBackend` delegates with explicit exhaustive matches and
receiver methods. By contrast, `SketchContractKit` has one concrete operation
owner, so its public receiver methods stay direct: do not add a backend trait,
inner enum, or forwarding facade without a second real implementation.

For a justified family, private dispatch and build/config-selection enums are
routing mechanisms only. They must not become reusable family identity,
provenance, capability, diagnostic-label, or error-label utilities. Keep the
trait hand-written and implemented by every member. Dispatcher arms call
receiver methods on trait-implementing payloads; payload UFCS such as
`RustExtractionBackend::generate(backend, context)` is invalid here. Do not
replace the contract with inherent-method parity by convention, trait objects,
public facade traits, compatibility shims, macros, generated dispatch, or
generated tables. Shared contract modules may define only
implementation-agnostic traits, shared handles, and default helpers; concrete
impl blocks live in the owning implementation subtree. An exported root handle
may use `<Self as Trait>::method(self, ...)` only to enter its own private trait
impl and avoid same-name inherent-method recursion; payload dispatch remains
receiver-style.

Private inner enum variants should wrap concrete implementation structs, and
dispatcher methods should delegate through explicit exhaustive `match` arms.
Prefer this over trait objects when the implementation set is closed and
known. Keep family-specific facts in the owning payload or documented
data/config type instead of adding shared `*ParserKind`, `*RunnerKind`,
display-name helpers, provenance fields, or capability-label routers.

## 5. Use lifetimes only when the borrow relationship matters

### Most functions do not need explicit lifetime syntax

```rust
pub fn normalize_username(input: &str) -> String {
    input.trim().to_lowercase()
}
```

Lifetime elision already covers this.

### Add explicit lifetimes when the output borrows from an input

```rust
pub fn first_word<'a>(input: &'a str) -> &'a str {
    input.split_whitespace().next().unwrap_or("")
}
```

### Use owned data in long-lived structs and spawned tasks

Bad:

```rust
pub struct UserService<'a> {
    api_base: &'a str,
}
```

This couples the service lifetime to whoever created it.

Better:

```rust
pub struct UserService {
    api_base: String,
}
```

### Borrowing views can still be great

```rust
#[derive(Debug)]
pub struct UserView<'a> {
    pub id: u64,
    pub email: &'a str,
}

pub fn view_user(user: &User) -> UserView<'_> {
    UserView {
        id: user.id,
        email: &user.email,
    }
}

#[derive(Debug)]
pub struct User {
    pub id: u64,
    pub email: String,
}
```

Use explicit lifetimes for short-lived views, parsers, and zero-copy helpers. Use owned data for long-lived services and tasks.

## 6. Prefer typed errors in libraries

### Good library error

```rust
#[derive(Debug, thiserror::Error)]
pub enum ParseAmountError {
    #[error("amount must be positive")]
    NonPositive,
    #[error("invalid number: {0}")]
    InvalidNumber(#[from] std::num::ParseFloatError),
}

pub fn parse_amount(input: &str) -> Result<f64, ParseAmountError> {
    let value: f64 = input.parse()?;
    if value <= 0.0 {
        return Err(ParseAmountError::NonPositive);
    }
    Ok(value)
}
```

Typed errors:

- preserve structure
- compose well
- document failure modes
- let callers branch intentionally

### Use `anyhow` at application boundaries

```rust
pub fn run_cli(input: &str) -> anyhow::Result<()> {
    let amount = parse_amount(input)
        .map_err(|error| anyhow::anyhow!("failed to parse amount from CLI input: {error}"))?;
    println!("{amount}");
    Ok(())
}
```

Use `anyhow` in binaries, top-level commands, or application orchestration. Avoid returning `anyhow::Result` from reusable library APIs unless the task explicitly wants that trade-off.

## 7. Panic only for bugs or impossible invariants

### Acceptable panic use

- impossible internal invariant
- test code
- examples
- one-time process bootstrap where failure means the program cannot continue anyway

### Prefer informative `expect` messages

Bad:

```rust
let config = CONFIG.get().unwrap();
```

Better:

```rust
let config = CONFIG
    .get()
    .expect("CONFIG must be initialized before request handling starts");
```

### Prefer `Result` for recoverable failures

If the caller could reasonably react, return `Result`.

## 8. Keep unsafe blocks tiny and behind safe abstractions

### Good shape

```rust
pub struct NonNullSlice<'a> {
    ptr: std::ptr::NonNull<u8>,
    len: usize,
    _marker: std::marker::PhantomData<&'a [u8]>,
}

impl<'a> NonNullSlice<'a> {
    pub fn from_slice(slice: &'a [u8]) -> Option<Self> {
        let ptr = std::ptr::NonNull::new(slice.as_ptr() as *mut u8)?;
        Some(Self {
            ptr,
            len: slice.len(),
            _marker: std::marker::PhantomData,
        })
    }

    pub fn as_slice(&self) -> &'a [u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.len) }
    }
}
```

Only the operation that actually requires `unsafe` is unsafe. The rest of the API is safe and documented.

### Avoid blanket guidance like this in a generic bundle

```toml
[lints.rust]
unsafe_code = "forbid"
```

That can be fine for many application crates, but it is too absolute as universal advice. FFI, low-level abstractions, and performance-sensitive internals may need narrow, audited `unsafe`.

### In Rust 2024-era code, also watch for

- `unsafe_op_in_unsafe_fn`
- newly unsafe environment mutation APIs such as `std::env::set_var` and `remove_var`

When touching these areas, audit them deliberately.

## 9. Abstraction dos and don'ts

### Do

- start concrete
- refactor repeated data + behavior into structs
- add traits only at real boundaries
- use enums for closed sets
- use explicit lifetimes only when the relationship matters
- keep library errors typed
- keep unsafe blocks tiny

### Don't

- create traits for speculative future reuse
- use generics where a concrete type is clearer
- return `anyhow::Result` from every library function by habit
- use panics for normal validation failures
- store references in long-lived services unless that lifetime coupling is intentional
- spread `unsafe` across large regions of code

## Further reading

- Rust book on generics and traits: <https://doc.rust-lang.org/book/ch10-00-generics.html>
- Rust book on lifetimes: <https://doc.rust-lang.org/book/ch10-03-lifetime-syntax.html>
- Rust book on error handling: <https://doc.rust-lang.org/book/ch09-00-error-handling.html>
- Rust book on unsafe Rust: <https://doc.rust-lang.org/book/ch20-01-unsafe-rust.html>
- Rustonomicon: <https://doc.rust-lang.org/nomicon/>


## 10. Additional merged examples

### Refactor once repetition is real

Bad:

```rust
pub fn cli_total(subtotal_cents: u64, tax_bps: u32) -> u64 {
    subtotal_cents + (subtotal_cents * tax_bps as u64 / 10_000)
}

pub fn http_total(subtotal_cents: u64, tax_bps: u32) -> u64 {
    subtotal_cents + (subtotal_cents * tax_bps as u64 / 10_000)
}
```

Better:

```rust
#[derive(Debug, Clone, Copy)]
pub struct MoneyPolicy {
    tax_bps: u32,
}

impl MoneyPolicy {
    pub fn new(tax_bps: u32) -> Self {
        Self { tax_bps }
    }

    pub fn total_cents(self, subtotal_cents: u64) -> u64 {
        subtotal_cents + (subtotal_cents * self.tax_bps as u64 / 10_000)
    }
}
```

Make the refactor when repeated logic or repeated invariants are real, not before.

### Keep justified enum dispatch explicit in Contract Kit

For a proven closed family, do not replace its hand-written trait and enum
dispatch with `macro_rules!`, proc-macro indirection, generated tables, or
codegen-style hidden abstraction. Contract Kit's current example is the
private syntax/compiler `RustExtractionBackend` family. Keep that dispatcher
explicit so reviewers can see each supported extraction path at the operation
boundary. Keep `SketchContractKit` direct because it does not have a second
concrete implementation. Trait definitions stay in implementation-agnostic
contract modules, while concrete impl blocks stay in the owning implementation
subtree. Use `enum-dispatch-trait-pattern.md` for the full decision checklist,
sync example, and native async trait example.

When forwarding boilerplate grows, first improve the model underneath:

- group repeated data into named structs
- move validation and lowering onto methods
- use closed enums for closed sets
- use typed spec data for repeated constants and tables
- use small helper functions for local behavior that has no state

The goal is to delete duplicated rules by giving them an owner, not to hide
duplicated branches behind generated code.

## Additional source links

- Rust book: generic data types: <https://doc.rust-lang.org/book/ch10-01-syntax.html>
- Rust book: traits: <https://doc.rust-lang.org/book/ch10-02-traits.html>
- Rust book: lifetimes: <https://doc.rust-lang.org/book/ch10-03-lifetime-syntax.html>
- Rust book: error handling: <https://doc.rust-lang.org/book/ch09-00-error-handling.html>
- Rust book: to panic or not to panic: <https://doc.rust-lang.org/book/ch09-03-to-panic-or-not-to-panic.html>
- `thiserror` docs: <https://docs.rs/thiserror>
- `anyhow` docs: <https://docs.rs/anyhow>
- Rust API Guidelines checklist: <https://rust-lang.github.io/api-guidelines/checklist.html>
- Rust design patterns: <https://rust-unofficial.github.io/patterns/>
- Rust builder pattern: <https://rust-unofficial.github.io/patterns/patterns/creational/builder.html>
- Rust book on enums: <https://doc.rust-lang.org/book/ch06-01-defining-an-enum.html>
- Rust book on match exhaustiveness: <https://doc.rust-lang.org/book/ch06-02-match.html>
- Rust blog on async fn in traits: <https://blog.rust-lang.org/2023/12/21/async-fn-rpit-in-traits/>
