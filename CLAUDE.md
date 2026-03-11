# CLAUDE.md - Manta Project

## Rust Best Practices

### Code Style & Formatting
- Follow the official Rust style guide (`cargo fmt`)
- Maximum line length: 100 characters
- Use `cargo clippy` for linting and fix all warnings
- Enable `#![deny(unsafe_code)]` where possible

### Error Handling
- Use `thiserror` for defining error types
- Use `anyhow` for application-level error handling
- Prefer `Result<T, E>` over panics
- Use `?` operator for error propagation
- Provide descriptive error messages with context

### Naming Conventions
- `PascalCase` for types, traits, enums, structs
- `snake_case` for functions, variables, modules
- `SCREAMING_SNAKE_CASE` for constants, statics
- `PascalCase` for enum variants and type parameters

### Documentation
- Document all public APIs with `///`
- Include examples in doc comments
- Use `cargo doc` to verify documentation builds
- Add module-level documentation with `//!`

### Testing
- Write unit tests in the same file (`#[cfg(test)] mod tests`)
- Use `cargo test` for running tests
- Aim for >80% code coverage
- Use `tokio::test` for async tests
- Use `mockall` for mocking dependencies

### Async Programming
- Use `tokio` as the async runtime
- Prefer `async/await` syntax
- Avoid blocking operations in async contexts
- Use `tokio::sync` primitives for synchronization

### Project Structure
```
manta/
├── Cargo.toml
├── CLAUDE.md
├── src/
│   ├── main.rs          # Application entry point
│   ├── lib.rs           # Library exports
│   ├── config.rs        # Configuration management
│   ├── cli.rs           # CLI argument parsing
│   ├── error.rs         # Error types
│   ├── core/            # Core business logic
│   │   ├── mod.rs
│   │   ├── models.rs    # Domain models
│   │   └── engine.rs    # Core engine
│   ├── adapters/        # External adapters
│   │   ├── mod.rs
│   │   ├── storage.rs   # Storage implementations
│   │   └── api.rs       # API clients
│   └── utils/           # Utilities
│       ├── mod.rs
│       └── logging.rs   # Logging setup
└── tests/               # Integration tests
    └── integration_tests.rs
```

### Dependencies
- Keep dependencies minimal and justified
- Use workspace dependencies for multi-crate projects
- Pin critical dependencies to specific versions
- Regularly run `cargo audit` for security

### Performance
- Use `cargo bench` for benchmarking
- Profile before optimizing
- Prefer zero-copy where possible
- Use `Arc<str>` over `String` for shared immutable strings
- Leverage iterators and lazy evaluation

### Safety
- Minimize use of `unsafe` code
- Document and justify all `unsafe` blocks
- Use safe abstractions where possible
- Run `miri` for undefined behavior detection

## Development Workflow

1. **Before committing:**
   - Run `cargo fmt`
   - Run `cargo clippy -- -D warnings`
   - Run `cargo test`
   - Run `cargo doc` to check docs build

2. **CI Checks:**
   - Format check: `cargo fmt -- --check`
   - Linting: `cargo clippy -- -D warnings`
   - Tests: `cargo test --all-features`
   - Documentation: `cargo doc --no-deps`
   - Security audit: `cargo audit`
