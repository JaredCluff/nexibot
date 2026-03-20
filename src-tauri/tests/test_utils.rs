//! Test utilities and helpers for comprehensive testing

#[cfg(test)]
pub mod test_helpers {
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Test database helper
    pub struct TestDatabase {
        dir: TempDir,
        path: PathBuf,
    }

    impl TestDatabase {
        pub fn new(name: &str) -> std::io::Result<Self> {
            let dir = TempDir::new()?;
            let path = dir.path().join(format!("{}.db", name));
            Ok(Self { dir, path })
        }

        pub fn path(&self) -> &PathBuf {
            &self.path
        }

        pub fn create(&self) -> std::io::Result<()> {
            std::fs::File::create(&self.path)?;
            Ok(())
        }

        pub fn cleanup(self) {
            // TempDir automatically cleans up on drop
            drop(self.dir);
        }
    }

    /// Mock service helper
    pub struct MockService {
        name: String,
        response_time_ms: u32,
    }

    impl MockService {
        pub fn new(name: &str, response_time_ms: u32) -> Self {
            Self {
                name: name.to_string(),
                response_time_ms,
            }
        }

        pub fn get_response_time(&self) -> u32 {
            self.response_time_ms
        }

        pub fn get_name(&self) -> &str {
            &self.name
        }
    }

    /// Test data generator
    pub struct TestDataGenerator;

    impl TestDataGenerator {
        /// Generate test memory content
        pub fn generate_memory_content(index: usize) -> String {
            format!("Test memory {} with some content and metadata", index)
        }

        /// Generate test API keys
        pub fn generate_api_key(index: usize) -> String {
            format!("sk-test-key-{:08}", index)
        }

        /// Generate test user ID
        pub fn generate_user_id(index: usize) -> String {
            format!("user-{:08}", index)
        }

        /// Generate test family name
        pub fn generate_family_name(index: usize) -> String {
            format!("Test Family {}", index)
        }

        /// Generate test email
        pub fn generate_email(index: usize) -> String {
            format!("user{}@example.com", index)
        }
    }

    /// Assertion helpers
    pub mod assertions {
        /// Assert value is between min and max
        pub fn assert_in_range(value: f32, min: f32, max: f32, msg: &str) {
            assert!(
                value >= min && value <= max,
                "{}: {} should be between {} and {}",
                msg,
                value,
                min,
                max
            );
        }

        /// Assert all values in range
        pub fn assert_all_in_range(values: &[f32], min: f32, max: f32, msg: &str) {
            for (i, &value) in values.iter().enumerate() {
                assert!(
                    value >= min && value <= max,
                    "{}: values[{}]={} should be between {} and {}",
                    msg,
                    i,
                    value,
                    min,
                    max
                );
            }
        }

        /// Assert list contains item
        pub fn assert_contains<T: PartialEq>(list: &[T], item: &T, msg: &str) {
            assert!(list.contains(item), "{}: item not found", msg);
        }

        /// Assert list is sorted
        pub fn assert_sorted<T: Ord + Clone + std::fmt::Debug>(list: &[T], msg: &str) {
            let mut sorted = list.to_vec();
            sorted.sort();
            assert_eq!(list, &sorted[..], "{}: list is not sorted", msg);
        }
    }

    /// Performance testing helpers
    pub struct PerformanceTimer {
        start: std::time::Instant,
        name: String,
    }

    impl PerformanceTimer {
        pub fn start(name: &str) -> Self {
            Self {
                start: std::time::Instant::now(),
                name: name.to_string(),
            }
        }

        pub fn elapsed_ms(&self) -> u128 {
            self.start.elapsed().as_millis()
        }

        pub fn finish(self) {
            println!(
                "[PERF] {}: {} ms",
                self.name,
                self.start.elapsed().as_millis()
            );
        }

        pub fn assert_less_than(self, max_ms: u128) {
            let elapsed = self.elapsed_ms();
            assert!(
                elapsed < max_ms,
                "[PERF] {} took {} ms, expected < {} ms",
                self.name,
                elapsed,
                max_ms
            );
            self.finish();
        }
    }

    /// Data verification helpers
    pub struct DataValidator;

    impl DataValidator {
        pub fn validate_memory_id(id: &str) -> bool {
            // UUIDs are typically 36 characters
            id.len() == 36
        }

        pub fn validate_importance_score(score: f32) -> bool {
            score >= 0.0 && score <= 100.0
        }

        pub fn validate_email(email: &str) -> bool {
            email.contains('@') && email.contains('.')
        }

        pub fn validate_timestamp(timestamp: &str) -> bool {
            // ISO 8601 format check
            timestamp.len() >= 20 && timestamp.contains('T')
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn test_data_generator() {
            let memory = TestDataGenerator::generate_memory_content(1);
            assert!(!memory.is_empty());

            let key = TestDataGenerator::generate_api_key(1);
            assert!(key.starts_with("sk-test-key"));

            let email = TestDataGenerator::generate_email(1);
            assert!(email.contains("@example.com"));
        }

        #[test]
        fn test_validator() {
            assert!(DataValidator::validate_email("user@example.com"));
            assert!(!DataValidator::validate_email("invalid.email"));
            assert!(DataValidator::validate_importance_score(50.0));
            assert!(!DataValidator::validate_importance_score(150.0));
        }

        #[test]
        fn test_assertions() {
            assertions::assert_in_range(50.0, 0.0, 100.0, "test");
            assertions::assert_all_in_range(&[10.0, 50.0, 90.0], 0.0, 100.0, "test");
        }
    }
}
