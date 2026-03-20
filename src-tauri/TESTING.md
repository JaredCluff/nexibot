# NexiBot Comprehensive Testing Guide

## Overview

This document outlines the comprehensive testing strategy for all new NexiBot systems. The test suite includes unit tests, integration tests, and end-to-end scenarios covering all major features.

## Test Organization

```
tests/
├── test_utils.rs              # Shared test utilities and helpers
├── integration_tests.rs       # System integration tests
├── db_maintenance_tests.rs    # Database maintenance & backup tests
├── memory_advanced_tests.rs   # Advanced memory feature tests
├── family_mode_tests.rs       # Multi-user/family mode tests
├── key_rotation_tests.rs      # API key rotation tests
├── dashboard_tests.rs         # Dashboard monitoring tests
└── e2e_scenarios.rs           # End-to-end integration scenarios
```

## Test Categories

### 1. Database Maintenance Tests (db_maintenance_tests.rs)
**Coverage:**
- Backup creation and metadata
- Backup restoration and recovery
- Database health checks
- Retention policy enforcement
- VACUUM and ANALYZE operations
- Concurrent backup operations
- Edge cases and error handling

**Key Test Cases:** 10+
- Backup lifecycle (create → verify → restore → delete)
- Health check corruption detection
- Automatic retention cleanup
- WAL file handling

### 2. Memory Advanced Tests (memory_advanced_tests.rs)
**Coverage:**
- Importance scoring and calculation
- Memory relationships and linking
- Duplicate detection via similarity
- Search with filters
- TTL-based expiration
- Export/import functionality
- Analytics generation
- Concurrent operations

**Key Test Cases:** 15+
- Importance scoring (critical, high, normal, low)
- Memory linking with all relationship types
- Similarity detection thresholds
- Search filtering (importance, verification, content)
- TTL expiration and cleanup
- Importance persistence across updates

### 3. Family Mode Tests (family_mode_tests.rs)
**Coverage:**
- Family creation and management
- User role-based access control
- Invitation lifecycle (create, accept, expire)
- Shared memory pools
- Activity logging
- Permission enforcement
- User management (add, remove, role change)

**Key Test Cases:** 20+
- Role permissions (Admin, Parent, User, Guest)
- Invitation expiry and rejection
- Memory access control by role
- Activity log limits and retention
- Concurrent operations
- Role hierarchy enforcement

### 4. Key Rotation Tests (key_rotation_tests.rs)
**Coverage:**
- API key management (add, activate, rotate)
- Key expiry detection and warnings
- Fallback mechanism
- Rotation scheduling
- Audit logging
- Usage tracking
- Multiple providers
- Concurrent operations

**Key Test Cases:** 20+
- Key rotation workflow
- Fallback to backup key
- Expiry warning generation
- Custom providers
- Usage statistics
- Audit trail verification

### 5. Dashboard Tests (dashboard_tests.rs)
**Coverage:**
- System metrics tracking (CPU, memory, disk)
- Service health monitoring
- Alert creation and management
- Message throughput tracking
- API latency monitoring
- Error rate tracking
- Historical data retention
- Concurrent updates

**Key Test Cases:** 20+
- Metrics calculation accuracy
- Service status transitions
- Alert severity levels
- 24-hour rolling window
- Concurrent metric updates
- Extreme value handling

### 6. Integration Tests (integration_tests.rs)
**Coverage:**
- Memory and dashboard interaction
- Key rotation and config system
- Family mode and memory pools
- Database maintenance and backup
- Concurrent access patterns

**Key Test Cases:** 5+
- Full subsystem interactions
- Data flow between systems
- Shared state management

### 7. End-to-End Scenarios (e2e_scenarios.rs)
**Coverage:**
- Complete workflows across systems
- High-volume stress tests
- Error recovery scenarios
- Data migration and persistence
- System scalability
- Permission verification

**Key Scenarios:** 15+
1. **Backup/Restore Cycle** - Full backup creation and recovery
2. **Family Collaboration** - Multi-user shared memory
3. **Key Rotation with Fallback** - Automated key management
4. **Intelligent Memory** - Importance and relationships
5. **Dashboard Monitoring** - Metric collection and display
6. **Concurrent Operations** - Multi-system parallelism
7. **Persistence/Recovery** - Data survives shutdown
8. **Memory Lifecycle** - Importance decay and eviction
9. **Multi-Family** - Isolation and separation
10. **Complete Workflow** - Full system integration
11. **High Volume Stress** - 1000+ operations
12. **Error Recovery** - Graceful degradation
13. **Access Control** - Permission verification
14. **Data Migration** - Export/import integrity
15. **Scalability** - Performance under load

## Test Utilities (test_utils.rs)

### TestDatabase
- Temporary database creation
- Automatic cleanup
- Path management

### MockService
- Service simulation
- Response time tracking

### TestDataGenerator
- Memory content generation
- API key generation
- User ID generation
- Email generation
- Family name generation

### DataValidator
- Memory ID validation
- Importance score range check
- Email format validation
- Timestamp validation

### PerformanceTimer
- Operation timing
- Performance assertions
- Automatic reporting

### Assertions
- Range validation
- List containment
- Sorted list verification

## Running Tests

### Run all tests
```bash
cd nexibot
cargo test
```

### Run specific test file
```bash
cargo test --test db_maintenance_tests
cargo test --test memory_advanced_tests
cargo test --test family_mode_tests
cargo test --test key_rotation_tests
cargo test --test dashboard_tests
cargo test --test integration_tests
cargo test --test e2e_scenarios
```

### Run tests with output
```bash
cargo test -- --nocapture
```

### Run tests single-threaded (for SQLite)
```bash
cargo test -- --test-threads=1
```

### Run specific test
```bash
cargo test test_backup_lifecycle -- --exact
```

### Run with backtrace on failure
```bash
RUST_BACKTRACE=1 cargo test
```

### Benchmark tests
```bash
cargo test -- --ignored --test-threads=1
```

## Test Coverage

### Current Coverage Targets
- **Database Maintenance:** 95%+ (critical for data safety)
- **Memory Advanced:** 90%+ (complex logic, needs thorough testing)
- **Family Mode:** 90%+ (access control is critical)
- **Key Rotation:** 90%+ (security-sensitive)
- **Dashboard:** 85%+ (monitoring accuracy)
- **Integration:** 80%+ (system interactions)
- **E2E Scenarios:** Representative coverage

### Coverage Commands
```bash
# Install tarpaulin
cargo install cargo-tarpaulin

# Generate coverage report
cargo tarpaulin --out Html --output-dir coverage

# View coverage
open coverage/index.html
```

## Test Execution Strategy

### Phase 1: Unit Tests
1. Run all unit tests in isolation
2. Verify individual component functionality
3. Check edge cases and error conditions
4. Validate data structures

### Phase 2: Integration Tests
1. Test component interactions
2. Verify data flow between systems
3. Check shared state management
4. Validate concurrent access

### Phase 3: End-to-End Scenarios
1. Test complete workflows
2. Simulate real-world usage
3. Performance profiling
4. Error recovery verification

### Phase 4: Stress and Performance Tests
1. High-volume data operations
2. Concurrent user scenarios
3. Memory and resource usage
4. Performance regression detection

## Debugging Failed Tests

### Enable logging
```bash
RUST_LOG=debug cargo test -- --nocapture
```

### Run single test with verbose output
```bash
cargo test test_name -- --nocapture --test-threads=1
```

### Use debugger
```bash
rust-lldb ./target/debug/deps/integration_tests-<hash>
```

## Continuous Integration

### GitHub Actions Workflow
```yaml
on: [push, pull_request]
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions-rs/toolchain@v1
      - run: cargo test --verbose
      - run: cargo tarpaulin --out Xml
```

## Test Maintenance

### Best Practices
1. **Keep tests independent** - No test should depend on another
2. **Use fixtures** - Create reusable test data setup
3. **Clean up** - Use TempDir for automatic cleanup
4. **Clear naming** - Test names describe what they test
5. **Single responsibility** - Each test verifies one thing
6. **Fast execution** - Keep tests quick
7. **Avoid sleep()** - Use deterministic test data
8. **Document assumptions** - Explain non-obvious test logic

### Common Pitfalls to Avoid
- ❌ Tests that depend on execution order
- ❌ Using real files instead of TempDir
- ❌ Hardcoded paths or environment assumptions
- ❌ Non-deterministic tests (random data)
- ❌ Overly broad assertions
- ❌ Mixing test logic with utility code
- ❌ Ignoring test failures

## Performance Baselines

### Expected Performance
- Database backup: < 100ms per GB
- Memory search: < 50ms for 50K entries
- Key rotation: < 20ms
- Dashboard aggregation: < 100ms
- Concurrent operations: No degradation

### Performance Monitoring
```rust
let timer = PerformanceTimer::start("operation_name");
// ... perform operation ...
timer.assert_less_than(100); // Assert < 100ms
```

## Security Testing

### Key Rotation Security
- ✓ Old keys disabled after rotation
- ✓ Audit trail of all operations
- ✓ No key exposure in logs
- ✓ Fallback mechanism works

### Family Mode Security
- ✓ Role-based access enforced
- ✓ Admin cannot be removed
- ✓ Invitation tokens are unique
- ✓ Expiry is enforced

### Memory Security
- ✓ No sensitive data in logs
- ✓ Importance doesn't leak info
- ✓ Relationships don't reveal secrets
- ✓ Export format is secure

## Troubleshooting

### Tests hang
- Check for deadlocks in Arc<RwLock<>>
- Verify async code isn't blocking
- Use `--test-threads=1` to isolate

### Database locked errors
- Use `--test-threads=1` for SQLite tests
- Verify TempDir cleanup
- Check file descriptor limits

### Flaky tests
- Avoid system time dependencies
- Use deterministic test data
- Increase timeouts if needed
- Check for race conditions

## Future Enhancements

- [ ] Property-based testing with quickcheck
- [ ] Mutation testing
- [ ] Load testing with criterion
- [ ] Continuous performance tracking
- [ ] ASLR and security fuzzing
- [ ] Code coverage reporting in CI
