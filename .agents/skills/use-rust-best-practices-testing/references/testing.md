# Rust testing best practices

Use this reference when choosing test levels, placing tests, or wiring live dependencies.

## Contents

- [Use the smallest effective level](#1-use-the-smallest-effective-test-level)
- [Keep unit tests next to code](#2-unit-tests-belong-next-to-the-code)
- [Use public APIs and real integration dependencies](#3-integration-tests-should-use-the-public-api-and-real-dependencies)
- [Use Compose or dev containers for multi-service tests](#4-use-docker-compose-or-dev-containers-for-multi-service-integration-and-e2e)
- [Add binary smoke tests](#5-add-binary-smoke-tests-for-cli-behavior)
- [Use doctests for public APIs](#6-use-doctests-for-public-apis)
- [Enable rustdoc link hygiene](#7-turn-on-rustdoc-link-hygiene-for-libraries)
- [Use a dedicated e2e crate](#8-use-a-dedicated-workspace-crate-for-e2e)
- [Make async tests deterministic](#9-make-async-tests-deterministic)
- [Test enum-dispatch structure](#10-add-structural-tests-for-enum-dispatch-families)
- [Review testing practices](#11-testing-dos-and-donts)
- [Further reading](#further-reading)
- [Read additional examples](#12-additional-merged-examples)
- [Additional source links](#additional-source-links)

## 1. Use the smallest effective test level

### Unit tests

Use unit tests for:

- pure functions
- small state machines
- validation logic
- branch coverage
- fake or mock interactions for dependencies you own

Put them in the same file as the code they test.

Keep test-only helpers inside the file-local `#[cfg(test)] mod tests` block or
under crate-level `tests/`. Do not put `#[cfg(test)]` on production-scope
impls, methods, imports, fields, trait impls, or constructors to make tests
easier to write. Production modules should expose the same type and method
shape in test and non-test builds.

### Integration tests

Use integration tests for:

- public API behavior of a package
- real database / cache / message broker dependencies
- request/response or repository contracts
- compiled binaries or process-level smoke tests

Put them in `tests/*.rs` inside the package being tested.

### Doctests

Use doctests for:

- public APIs that benefit from executable examples
- library usage snippets
- compile-fail or no-run documentation examples

Put them in doc comments on the public item.

### Binary smoke tests

Use them for:

- CLI help output
- config parsing
- basic process startup and error messages

Put them in `tests/*.rs` and invoke the compiled binary through `CARGO_BIN_EXE_<name>`.

### End-to-end tests

Use e2e tests for:

- full-system flows across multiple crates and services
- external contract validation
- live dependencies and actual process boundaries

Keep them in a dedicated workspace crate or at the main app boundary.

## 2. Unit tests belong next to the code

### Good: file-local unit tests with a fake

```rust
pub trait Clock {
    fn unix_seconds(&self) -> u64;
}

pub struct SessionService<C> {
    clock: C,
}

impl<C: Clock> SessionService<C> {
    pub fn new(clock: C) -> Self {
        Self { clock }
    }

    pub fn issue_expiry(&self, ttl_seconds: u64) -> u64 {
        self.clock.unix_seconds() + ttl_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    struct FakeClock {
        now: u64,
    }

    impl Clock for FakeClock {
        fn unix_seconds(&self) -> u64 {
            self.now
        }
    }

    #[test]
    fn computes_expiry_from_fake_clock() {
        let service = SessionService::new(FakeClock { now: 1_700_000_000 });
        assert_eq!(service.issue_expiry(300), 1_700_000_300);
    }
}
```

### Bad

- reaching the network
- talking to the real database
- sleeping for arbitrary durations
- requiring Docker or external credentials
- adding production-scope `#[cfg(test)]` shims such as test-only `From` impls,
  constructors, accessors, or imports outside a local test module

## 3. Integration tests should use the public API and real dependencies

### Prefer `testcontainers-modules` when available

When a maintained module exists, it is usually easier than building a raw image request yourself.

```rust
use sqlx::PgPool;
use testcontainers_modules::{postgres, testcontainers::runners::SyncRunner};

#[test]
fn repository_can_connect_to_postgres() -> Result<(), Box<dyn std::error::Error>> {
    let container = postgres::Postgres::default().start()?;
    let host_port = container.get_host_port_ipv4(5432)?;

    let database_url = format!(
        "postgres://postgres:postgres@127.0.0.1:{host_port}/postgres"
    );

    tokio::runtime::Runtime::new()?.block_on(async {
        let pool = PgPool::connect(&database_url).await?;
        let row: (i64,) = sqlx::query_as("SELECT 1").fetch_one(&pool).await?;
        assert_eq!(row.0, 1);
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;

    Ok(())
}
```

### `GenericImage` fallback remains valid

Use it when no maintained community module exists or when you need a very specific custom image.

```rust
use std::error::Error;
use testcontainers::{
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
    GenericImage, ImageExt,
};

#[tokio::test]
async fn redis_round_trip() -> Result<(), Box<dyn Error>> {
    let container = GenericImage::new("redis", "7.2.4")
        .with_exposed_port(6379.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await?;

    let host_port = container.get_host_port_ipv4(6379).await?;
    let client = redis::Client::open(format!("redis://127.0.0.1:{host_port}/"))?;
    let mut conn = client.get_multiplexed_async_connection().await?;

    let _: () = redis::cmd("SET")
        .arg("greeting")
        .arg("hello")
        .query_async(&mut conn)
        .await?;

    let value: String = redis::cmd("GET")
        .arg("greeting")
        .query_async(&mut conn)
        .await?;

    assert_eq!(value, "hello");
    Ok(())
}
```

## 4. Use Docker Compose or dev containers for multi-service integration and e2e

When a test needs several services together, a compose file can be clearer than hand-wiring several raw containers.

```rust
use testcontainers::compose::DockerCompose;

#[tokio::test]
async fn stack_healthcheck_passes() -> Result<(), Box<dyn std::error::Error>> {
    let mut compose = DockerCompose::with_local_client(&["tests/docker-compose.yml"]);
    compose.up().await?;

    let web_port = compose.get_host_port_ipv4("web", 8080).await?;
    let response = reqwest::get(format!("http://127.0.0.1:{web_port}/health"))
        .await?
        .text()
        .await?;

    assert_eq!(response, "OK");
    Ok(())
}
```

Use real test API keys through environment variables only when the repository already supports that workflow and the test boundary genuinely requires it.

## 5. Add binary smoke tests for CLI behavior

Cargo exposes compiled binaries to integration tests via `CARGO_BIN_EXE_<name>`.

```rust
use std::process::Command;

#[test]
fn help_output_mentions_config_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_my-app"))
        .arg("--help")
        .output()
        .expect("binary should run");

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert!(stdout.contains("--config"));
}
```

These tests are ideal for:

- `--help`
- missing required arguments
- config-file parsing
- startup banner or version output

They are not a replacement for full e2e tests.

## 6. Use doctests for public APIs

### Simple doctest

```rust
/// Parses a user id.
///
/// ```
/// use my_lib::parse_user_id;
/// assert_eq!(parse_user_id("42").unwrap(), 42);
/// ```
pub fn parse_user_id(input: &str) -> Result<u64, ParseUserIdError> {
    input.parse().map_err(ParseUserIdError::from)
}

#[derive(Debug, thiserror::Error)]
#[error("invalid user id")]
pub struct ParseUserIdError(#[from] std::num::ParseIntError);
```

### `compile_fail`

```rust
/// ```compile_fail
/// use my_lib::parse_user_id;
/// let _ = parse_user_id(42);
/// ```
```

### `no_run`

Use when the example should compile but should not execute in docs, such as examples that call external services.

```rust
/// ```no_run
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let body = reqwest::get("https://example.com").await?.text().await?;
/// println!("{body}");
/// # Ok(())
/// # }
/// ```
```

### `should_panic`

Use sparingly, mostly for deliberate panic examples.

```rust
/// ```should_panic
/// assert_eq!(2 + 2, 5);
/// ```
```

## 7. Turn on rustdoc link hygiene for libraries

At the crate root of a reusable library:

```rust
#![deny(rustdoc::broken_intra_doc_links)]
#![deny(rustdoc::private_intra_doc_links)]
```

This catches documentation links that silently rot.

## 8. Use a dedicated workspace crate for e2e

Example layout:

```text
tests/e2e/
├── Cargo.toml
└── src/
    ├── lib.rs
    └── signup_flow.rs
```

`tests/e2e/Cargo.toml`

```toml
[package]
name = "workspace-e2e"
version = "0.1.0"
edition.workspace = true

[lints]
workspace = true

[dependencies]
reqwest = { version = "0.12", features = ["json"] }
tokio.workspace = true
```

This keeps top-level system tests from polluting individual domain crates.

## 9. Make async tests deterministic

### Prefer readiness signals over arbitrary sleeps

Bad:

```rust
#[tokio::test]
async fn bad() {
    tokio::spawn(async move {
        start_server().await;
    });

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let response = reqwest::get("http://127.0.0.1:3000/health").await.unwrap();
    assert!(response.status().is_success());
}
```

Better:

```rust
#[tokio::test]
async fn waits_for_readiness_signal() {
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        start_server(ready_tx).await;
    });

    ready_rx.await.expect("server should signal readiness");
    let response = reqwest::get("http://127.0.0.1:3000/health")
        .await
        .expect("request should succeed");
    assert!(response.status().is_success());
}

async fn start_server(ready_tx: tokio::sync::oneshot::Sender<()>) {
    let _ = ready_tx.send(());
}
```

### Add explicit timeouts for test awaits

Wrap network or readiness waits in `tokio::time::timeout(...)` when a stuck test would otherwise hang forever.

## 10. Add structural tests for enum-dispatch families

When a crate uses closed enum dispatch over implementation families such as
contract parsers, signature matchers, sketch runners, or output emitters, test
the structure directly:

- the implementation-agnostic `pub(crate)` trait exists
- exported dispatchers use opaque public handles over private inner enums,
  while crate-private dispatch enums may remain plain private enums
- the public handle or private dispatcher enum implements that trait with
  explicit `match` arms
- every concrete implementation struct implements the trait
- enum arms use receiver-method calls on trait-implementing payloads, not
  `Trait::method(receiver, args)` dispatch-contract UFCS
- public root handle methods may use `<Self as Trait>::method(self, ...)` only
  to bridge into the handle's own private trait impl
- concrete impl blocks live in the owning implementation subtree
- shared contract modules do not name concrete implementation families
- wildcard or catch-all arms are absent from implementation-family dispatch
- macro-generated dispatch, `async_trait`, compatibility shims, and stale
  restoration TODO markers are absent

These tests complement behavioral tests. They make deletion of the parity
contract fail immediately instead of relying on reviewer memory.

## 11. Testing dos and don'ts

### Do

- test behavior at the narrowest useful level
- keep unit tests local and fast
- use real dependencies in integration/e2e tests
- guard enum-dispatch trait contracts with structural tests
- add doctests for public API examples
- add binary smoke tests for CLI behavior
- keep e2e tests at the actual system boundary

### Don't

- mock infrastructure you do not own in integration tests
- replace enum-dispatch structural tests with macro-generated checks
- use networked resources in unit tests
- place whole-system tests inside unrelated domain crates
- rely on arbitrary sleeps for async coordination
- duplicate the same scenario across every test layer

## Further reading

- Cargo test: <https://doc.rust-lang.org/cargo/commands/cargo-test.html>
- Cargo environment variables: <https://doc.rust-lang.org/cargo/reference/environment-variables.html>
- Rustdoc doctests: <https://doc.rust-lang.org/rustdoc/write-documentation/documentation-tests.html>
- Rustdoc lints: <https://doc.rust-lang.org/rustdoc/lints.html>
- Testcontainers for Rust: <https://rust.testcontainers.org/>


## 12. Additional merged examples

### Unit tests: trait + fake, no live dependencies

```rust
pub trait EmailSender {
    fn send_welcome(&self, email: &str) -> Result<(), String>;
}

pub struct SignupService<S> {
    sender: S,
}

impl<S: EmailSender> SignupService<S> {
    pub fn new(sender: S) -> Self {
        Self { sender }
    }

    pub fn signup(&self, email: &str) -> Result<(), String> {
        if !email.contains('@') {
            return Err("invalid email".into());
        }
        self.sender.send_welcome(email)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct FakeSender {
        sent: Arc<Mutex<Vec<String>>>,
    }

    impl EmailSender for FakeSender {
        fn send_welcome(&self, email: &str) -> Result<(), String> {
            self.sent.lock().unwrap().push(email.to_owned());
            Ok(())
        }
    }

    #[test]
    fn signup_sends_welcome_email() {
        let fake = FakeSender::default();
        let sent = fake.sent.clone();
        let service = SignupService::new(fake);

        service.signup("user@example.com").unwrap();

        assert_eq!(sent.lock().unwrap().as_slice(), ["user@example.com"]);
    }
}
```

### Integration tests can use live test API keys

```rust
#[tokio::test]
async fn invoices_api_healthcheck() -> anyhow::Result<()> {
    let base_url = std::env::var("TEST_API_BASE_URL")?;
    let api_key = std::env::var("TEST_API_KEY")?;

    let client = reqwest::Client::new();
    let response = client
        .get(format!("{base_url}/health"))
        .bearer_auth(api_key)
        .send()
        .await?;

    assert!(response.status().is_success());
    Ok(())
}
```

Use this pattern for real crate-level integrations when the repository intentionally supports live test credentials in CI, dev containers, or secure local workflows.

### Timer-heavy async tests can start with time paused

This requires Tokio's `test-util` feature.

```rust
#[tokio::test(start_paused = true)]
async fn retry_waits_before_next_attempt() {
    use std::time::{Duration, Instant};

    let start = Instant::now();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(start.elapsed() >= Duration::from_secs(5));
}
```

## Additional source links

- Rust book: test organization: <https://doc.rust-lang.org/book/ch11-03-test-organization.html>
- testcontainers crate docs: <https://docs.rs/testcontainers/latest/testcontainers/>
- testcontainers quickstart: <https://rust.testcontainers.org/quickstart/testcontainers/>
- testcontainers networking: <https://rust.testcontainers.org/features/networking/>
- testcontainers modules: <https://docs.rs/testcontainers-modules>
- Dev Containers spec: <https://containers.dev/>
- GitHub Codespaces project setup: <https://docs.github.com/en/codespaces/setting-up-your-project-for-codespaces>
- Tokio testing guide: <https://tokio.rs/tokio/topics/testing>
- `tokio::test` docs: <https://docs.rs/tokio/latest/tokio/attr.test.html>
