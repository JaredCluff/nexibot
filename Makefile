.PHONY: test test-all test-unit test-integration test-db test-memory test-family test-keys test-dashboard test-e2e coverage clean help

# Help target
help:
	@echo "NexiBot Testing Commands"
	@echo "========================"
	@echo ""
	@echo "make test              - Run all tests"
	@echo "make test-unit         - Run unit tests only"
	@echo "make test-integration  - Run integration tests"
	@echo "make test-db           - Test database maintenance"
	@echo "make test-memory       - Test advanced memory"
	@echo "make test-family       - Test family mode"
	@echo "make test-keys         - Test key rotation"
	@echo "make test-dashboard    - Test dashboard"
	@echo "make test-e2e          - Test end-to-end scenarios"
	@echo "make coverage          - Generate coverage report"
	@echo "make coverage-html     - Generate HTML coverage report"
	@echo "make clean             - Clean test artifacts"
	@echo "make test-watch        - Run tests on file changes"
	@echo "make test-verbose      - Run tests with output"
	@echo "make test-quiet        - Run tests quietly"
	@echo "make test-single       - Run tests single-threaded (for SQLite)"
	@echo ""

# Run all tests
test:
	@echo "Running all tests..."
	cargo test --verbose

# Run unit tests only
test-unit:
	@echo "Running unit tests..."
	cd src-tauri && cargo test --lib

# Run integration tests
test-integration:
	@echo "Running integration tests..."
	cargo test --test integration_tests

# Test database maintenance
test-db:
	@echo "Testing database maintenance..."
	cargo test --test db_maintenance_tests

# Test memory advanced features
test-memory:
	@echo "Testing advanced memory features..."
	cargo test --test memory_advanced_tests

# Test family mode
test-family:
	@echo "Testing family mode..."
	cargo test --test family_mode_tests

# Test key rotation
test-keys:
	@echo "Testing key rotation..."
	cargo test --test key_rotation_tests

# Test dashboard
test-dashboard:
	@echo "Testing dashboard..."
	cargo test --test dashboard_tests

# Test E2E scenarios
test-e2e:
	@echo "Testing end-to-end scenarios..."
	cargo test --test e2e_scenarios

# Run tests with output
test-verbose:
	@echo "Running tests with verbose output..."
	cargo test -- --nocapture --test-threads=1

# Run tests quietly
test-quiet:
	@echo "Running tests quietly..."
	cargo test --release

# Run tests single-threaded (for SQLite)
test-single:
	@echo "Running tests single-threaded..."
	cargo test -- --test-threads=1

# Watch for changes and rerun tests
test-watch:
	@echo "Watching for changes..."
	cargo watch -x test

# Generate coverage report (requires tarpaulin)
coverage:
	@echo "Generating coverage report..."
	@command -v cargo-tarpaulin >/dev/null 2>&1 || \
		(echo "Installing cargo-tarpaulin..." && cargo install cargo-tarpaulin)
	cargo tarpaulin --out Stdout

# Generate HTML coverage report
coverage-html:
	@echo "Generating HTML coverage report..."
	@command -v cargo-tarpaulin >/dev/null 2>&1 || \
		(echo "Installing cargo-tarpaulin..." && cargo install cargo-tarpaulin)
	cargo tarpaulin --out Html --output-dir coverage
	@echo "Coverage report generated in coverage/index.html"
	@if command -v open >/dev/null 2>&1; then open coverage/index.html; fi

# Run benchmark tests
bench:
	@echo "Running benchmark tests..."
	cargo test -- --ignored

# Clean test artifacts
clean:
	@echo "Cleaning test artifacts..."
	rm -rf coverage target/debug/deps/db_maintenance_tests*
	rm -rf target/debug/deps/memory_advanced_tests*
	rm -rf target/debug/deps/family_mode_tests*
	rm -rf target/debug/deps/key_rotation_tests*
	rm -rf target/debug/deps/dashboard_tests*
	rm -rf target/debug/deps/integration_tests*
	rm -rf target/debug/deps/e2e_scenarios*

# Run all tests with code coverage
test-coverage: coverage-html

# Run clippy lints
lint:
	@echo "Running clippy lints..."
	cargo clippy -- -D warnings

# Format code
fmt:
	@echo "Formatting code..."
	cargo fmt --all

# Check format
check-fmt:
	@echo "Checking code format..."
	cargo fmt --all -- --check

# Full test suite (lint + format + tests + coverage)
test-full: check-fmt lint test coverage-html
	@echo "Full test suite completed!"

# Run specific test by name
test-name:
	@read -p "Enter test name: " test_name; \
	cargo test $$test_name -- --nocapture

# Run tests with backtrace
test-backtrace:
	@echo "Running tests with backtrace..."
	RUST_BACKTRACE=1 cargo test -- --nocapture

# Run documentation tests
test-docs:
	@echo "Running documentation tests..."
	cargo test --doc

# Run all test variants
test-all: test-single test-verbose test-coverage lint
	@echo "All test variants completed!"

# Initialize test environment
test-init:
	@echo "Initializing test environment..."
	@command -v cargo-watch >/dev/null 2>&1 || \
		(echo "Installing cargo-watch..." && cargo install cargo-watch)
	@command -v cargo-tarpaulin >/dev/null 2>&1 || \
		(echo "Installing cargo-tarpaulin..." && cargo install cargo-tarpaulin)
	@echo "Test environment initialized!"

# Show test statistics
test-stats:
	@echo "Test Statistics"
	@echo "==============="
	@echo "Total test files: $$(ls tests/*.rs 2>/dev/null | wc -l)"
	@echo "Total test cases: $$(grep -r '#\[.*test\]' src-tauri/tests/ 2>/dev/null | wc -l)"
	@echo "Module tests: $$(grep -r '#\[.*test\]' src-tauri/src/ 2>/dev/null | wc -l)"
	@echo ""
	@echo "By module:"
	@grep -r '#\[.*test\]' src-tauri/src/ 2>/dev/null | \
		sed 's/:.*#.*//' | sort | uniq -c | sort -rn

.DEFAULT_GOAL := help
