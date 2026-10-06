//! The production generator's UUID/ULID contract, independent of HTTP serving.

use based_runtime::id::UuidGen;
use based_runtime::IdGen;

#[test]
fn production_uuid_is_random_v4() {
    let ids = UuidGen;
    let first = uuid::Uuid::parse_str(&ids.next_id()).unwrap();
    let second = uuid::Uuid::parse_str(&ids.next_id()).unwrap();
    assert_eq!(first.get_version(), Some(uuid::Version::Random));
    assert_eq!(first.get_variant(), uuid::Variant::RFC4122);
    assert_ne!(first, second);
}

#[test]
fn production_ulid_uses_the_ulid_strategy() {
    let ids = UuidGen;
    let first = ids.next_ulid();
    let parsed = ulid::Ulid::from_string(&first).unwrap();
    assert_eq!(parsed.to_string(), first);
    assert_ne!(first, ids.next_ulid());
}
