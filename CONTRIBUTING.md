# Contributing to NexiBot

Thanks for your interest in contributing. Here's how to get involved.

## Reporting Bugs

Open a [GitHub issue](https://github.com/jaredcluff/nexibot/issues) with:

- What you expected to happen
- What actually happened
- Steps to reproduce
- Your environment (OS, NexiBot version, relevant config)
- Logs if available (`RUST_LOG=debug`)

## Suggesting Features

Open a [GitHub issue](https://github.com/jaredcluff/nexibot/issues) with the `enhancement` label. Describe the problem you're trying to solve, not just the solution you want. Context helps us design the right thing.

## Development Setup

### Prerequisites

- **Rust** (stable, 1.75+): https://rustup.rs/
- **Node.js** (22.12+): Required for the Anthropic Bridge and UI
- **npm**: Comes with Node.js

### Build

```bash
# Clone
git clone https://github.com/jaredcluff/nexibot.git
cd nexibot

# Install dependencies
cd ui && npm install && cd ..
cd bridge && npm install && cd ..
cd bridge/plugins/anthropic && npm install && cd ../../..
cd bridge/plugins/openai && npm install && cd ../../..

# Desktop app (Tauri)
cargo tauri dev
```

### Linux Dependencies

```bash
# Ubuntu/Debian
sudo apt-get install -y \
    libgtk-3-dev \
    libwebkit2gtk-4.1-dev \
    libappindicator3-dev \
    librsvg2-dev \
    patchelf
```

## Code Style

- **Rust**: Run `cargo fmt` and `cargo clippy` before committing. Fix all warnings.
- **TypeScript**: Run ESLint on UI code (`cd ui && npx eslint .`).
- Keep functions short. Prefer clear names over comments.

## Pull Request Process

1. Fork the repo and create a branch from `main`:
   ```bash
   git checkout -b feat/my-thing
   ```

2. Make your changes. Write tests if the change is non-trivial.

3. Make sure everything passes:
   ```bash
   cargo test
   cargo clippy
   cargo fmt --check
   ```

4. Push and open a PR against `main`. Describe what changed, why, and how to test it.

5. Address review feedback. We'll merge once it looks good.

## What We're Looking For

- Bug fixes with regression tests
- Performance improvements with benchmarks
- New channel integrations
- Documentation improvements
- Platform-specific fixes (especially Windows and Linux)

## Feature Flags

The `connect` feature gates the optional Knowledge Nexus SaaS integration (`nexibot-connect` plugin). This is **not** part of the open source release. Building with `--features connect` requires the `nexibot-connect` crate cloned as a sibling directory. The default build (`cargo tauri dev` / `cargo tauri build`) does not require it.

## What to Avoid

- Large refactors without prior discussion -- open an issue first
- Changes that break offline/local-first functionality
- Adding heavy dependencies without justification

## License

By contributing, you agree that your contributions will be licensed under the Apache License 2.0.
