# Contributing to Manta

Thank you for your interest in contributing to Manta! This document provides guidelines and instructions for contributing.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [How to Contribute](#how-to-contribute)
- [Coding Standards](#coding-standards)
- [Commit Messages](#commit-messages)
- [Pull Request Process](#pull-request-process)

## Code of Conduct

This project and everyone participating in it is governed by our commitment to:
- Be respectful and inclusive
- Welcome newcomers and help them learn
- Focus on constructive criticism
- Accept responsibility and apologize when mistakes happen

## Getting Started

1. Fork the repository on GitHub
2. Clone your fork locally
3. Create a new branch for your feature or fix
4. Make your changes
5. Submit a pull request

## Development Setup

### Prerequisites

- Rust 1.75 or higher
- SQLite development libraries
- Git

### Build

```bash
# Clone the repository
git clone https://github.com/anthropics/manta
cd manta

# Build in development mode
cargo build

# Build with all features
cargo build --all-features

# Run tests
cargo test --all-features
```

### Code Formatting

Before submitting, ensure your code is properly formatted:

```bash
cargo fmt
cargo clippy --all-features -- -D warnings
```

## How to Contribute

### Reporting Bugs

When reporting bugs, please include:
- A clear description of the issue
- Steps to reproduce
- Expected behavior
- Actual behavior
- Environment details (OS, Rust version, etc.)
- Any relevant logs or error messages

### Suggesting Features

Feature suggestions are welcome! Please:
- Check if the feature has already been suggested
- Provide a clear use case
- Explain why it would be valuable
- Consider implementation complexity

### Contributing Code

#### Areas for Contribution

- **Skills**: Create new skills in `examples/skills/`
- **Tools**: Add new tools in `src/tools/`
- **Channels**: Implement new channel integrations
- **Documentation**: Improve docs and examples
- **Tests**: Add test coverage
- **Bug fixes**: Fix reported issues

#### Skill Contributions

To add a new skill:

1. Create a directory in `examples/skills/`
2. Add a `SKILL.md` file following the template
3. Test the skill with Manta
4. Submit a PR with examples of usage

Example skill structure:
```
examples/skills/my_skill/
├── SKILL.md          # Required: Skill definition
├── config.yaml       # Optional: Default configuration
└── README.md         # Optional: Additional documentation
```

## Coding Standards

### Rust Style

Follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/):
- Use `cargo fmt` for formatting
- Use `cargo clippy` for linting
- Write documentation for all public items
- Use meaningful variable names
- Keep functions focused and small

### Documentation

- Use `///` for documentation comments
- Include examples in doc comments
- Document panics, errors, and safety requirements
- Keep documentation up-to-date with code changes

### Testing

- Write unit tests for new functionality
- Use integration tests for complex features
- Aim for >80% code coverage
- Test edge cases and error conditions

Example test:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_my_feature() {
        let result = my_function("input");
        assert_eq!(result, "expected");
    }

    #[tokio::test]
    async fn test_async_feature() {
        let result = my_async_function().await;
        assert!(result.is_ok());
    }
}
```

## Commit Messages

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

Types:
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `style`: Code style changes (formatting)
- `refactor`: Code refactoring
- `perf`: Performance improvements
- `test`: Adding or updating tests
- `chore`: Build process or auxiliary tool changes

Examples:
```
feat(tools): add grep tool for searching files

fix(agent): resolve budget tracking issue
docs: update API documentation for channels
test(memory): add tests for session search
```

## Pull Request Process

1. **Before Submitting**
   - Ensure tests pass: `cargo test --all-features`
   - Check formatting: `cargo fmt -- --check`
   - Run clippy: `cargo clippy --all-features -- -D warnings`
   - Update documentation if needed
   - Add tests for new functionality

2. **PR Description**
   - Clearly describe the changes
   - Reference related issues with `Fixes #123` or `Relates to #456`
   - Include screenshots for UI changes
   - List breaking changes if any

3. **Review Process**
   - Maintainers will review within a few days
   - Address review comments promptly
   - Keep the PR focused on a single topic
   - Rebase on main if there are conflicts

4. **After Merge**
   - Your contribution will be in the next release
   - Thank you for helping improve Manta!

## Development Tips

### Running Specific Tests

```bash
# Run specific test
cargo test test_name

# Run tests in a module
cargo test module_name

# Run with output
cargo test -- --nocapture
```

### Debugging

```bash
# Enable debug logging
RUST_LOG=debug cargo run

# Enable trace logging
RUST_LOG=trace cargo run
```

### Feature Flags

Test with different feature combinations:

```bash
# Default features
cargo test

# All features
cargo test --all-features

# Specific features
cargo test --features "telegram discord"
```

## Questions?

- Join our [Discord](https://discord.gg/manta)
- Open a [GitHub Discussion](https://github.com/anthropics/manta/discussions)
- Check existing [Issues](https://github.com/anthropics/manta/issues)

## License

By contributing, you agree that your contributions will be licensed under the same license as the project (MIT OR Apache-2.0).

---

Thank you for contributing to Manta! 🎉
