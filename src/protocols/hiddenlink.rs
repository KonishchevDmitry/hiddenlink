pub const MIN_SECRET_LEN: u64 = 10;

pub const STATIC_HEADER_SIZE: usize = 1 + 1 + 2; // flags + name length + HTTP request size
pub const HEADER_SUFFIX: &[u8] = "hiddenlink!".as_bytes();

// The result secret should be a prefix of a first line of a valid HTTP request – in other case, in some corner cases an
// attacker might be able to brute-force the secret symbol-by-symbol if he knows how exactly the specific upstream server
// parses and validates HTTP requests.
pub fn encode_secret_for_http1(secret: &str) -> Vec<u8> {
    format!("POST /hiddenlink/v1/connect?secret={}", urlencoding::encode(secret)).into_bytes()
}