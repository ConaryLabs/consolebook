//! Golden vectors pinning the canonical record format (ADR 0011;
//! `docs/records-integrity.md`): member ordering, JCS string escaping,
//! integer-only numbers, the SHA-256 content hash, and the
//! domain-separated chain hash with the fixed missing-predecessor
//! treatment. These bytes are the specification; the serializer must
//! keep matching them. Every fixture is invented.

use consolebook_server::canonical;
use serde_json::json;

/// The vector document's canonical bytes, exactly: sorted members, no
/// whitespace, two-character escapes for `\n` and `\t`, lowercase
/// `\u0001` for the other control character, and raw UTF-8 for the
/// em dash, the euro sign, and the emoji.
const VECTOR_CANONICAL: &str = "{\"a\":\"Multi-line\\n\\tText — dash € 😀\",\"b\":[1,-2,null,true,false],\"n\":{\"x\":\"control\\u0001\",\"y\":\"\"}}";

const VECTOR_CONTENT_HASH: &str =
    "fe6ea6e53b76807928775df83e0850037155cd64939ec005b957c3cd402a1fa6";
const VECTOR_CHAIN_FIRST: &str = "8890fe17cb82a298cc221afa5af9699c73f0f6797da8bcf7071b27db4ce6c655";
const VECTOR_CHAIN_SUCCESSOR: &str =
    "2054a75167f5c7e612d0975368e1ad93a7139c8a46c64d8f7ba13c07ca3e95dc";

#[test]
fn canonicalization_matches_the_golden_vector() {
    // Members deliberately authored out of order: ordering comes from
    // the format, never from authoring order.
    let document = json!({
        "n": { "y": "", "x": "control\u{0001}" },
        "b": [1, -2, null, true, false],
        "a": "Multi-line\n\tText — dash € 😀",
    });
    let bytes = canonical::canonical_bytes(&document).expect("canonical");
    assert_eq!(
        std::str::from_utf8(&bytes).expect("utf-8"),
        VECTOR_CANONICAL
    );
}

#[test]
fn jcs_escapes_are_exact() {
    let document = json!({ "s": "\u{0008}\u{000c}\u{001f}" });
    let bytes = canonical::canonical_bytes(&document).expect("canonical");
    assert_eq!(
        std::str::from_utf8(&bytes).expect("utf-8"),
        "{\"s\":\"\\b\\f\\u001f\"}"
    );
}

#[test]
fn hash_vectors_hold() {
    let bytes = VECTOR_CANONICAL.as_bytes();
    assert_eq!(canonical::content_hash_hex(bytes), VECTOR_CONTENT_HASH);
    assert_eq!(
        canonical::chain_hash_hex(None, bytes).expect("chain"),
        VECTOR_CHAIN_FIRST
    );
    assert_eq!(
        canonical::chain_hash_hex(Some(VECTOR_CONTENT_HASH), bytes).expect("chain"),
        VECTOR_CHAIN_SUCCESSOR
    );
}

#[test]
fn the_closed_subset_refuses_what_the_format_forbids() {
    let float = canonical::canonical_bytes(&json!({ "value": 1.5 }));
    assert!(
        float
            .expect_err("floats are refused")
            .to_string()
            .contains("integers only")
    );

    let big = canonical::canonical_bytes(&json!({ "value": 9_007_199_254_740_992_i64 }));
    assert!(
        big.expect_err("2^53 is refused")
            .to_string()
            .contains("magnitude")
    );

    let key = canonical::canonical_bytes(&json!({ "clé": 1 }));
    assert!(
        key.expect_err("non-ASCII member names are refused")
            .to_string()
            .contains("non-ASCII")
    );

    // The boundary itself is representable.
    let edge = canonical::canonical_bytes(&json!({
        "max": 9_007_199_254_740_991_i64,
        "min": -9_007_199_254_740_991_i64,
    }))
    .expect("2^53 - 1 is representable");
    assert_eq!(
        std::str::from_utf8(&edge).expect("utf-8"),
        "{\"max\":9007199254740991,\"min\":-9007199254740991}"
    );
}

#[test]
fn a_bad_predecessor_hash_is_refused() {
    let short = canonical::chain_hash_hex(Some("abc123"), b"{}");
    assert!(
        short
            .expect_err("length checked")
            .to_string()
            .contains("64 hex characters")
    );
    let invalid = canonical::chain_hash_hex(Some(&"zz".repeat(32)), b"{}");
    assert!(
        invalid
            .expect_err("hex checked")
            .to_string()
            .contains("hex encoding")
    );
}
