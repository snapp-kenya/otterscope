use super::*;

#[test]
fn migrations_apply_and_are_idempotent() {
    let dir = std::env::temp_dir().join(format!("otterscope-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.db");
    let path = path.to_str().unwrap();

    let store = Store::open(path).unwrap();
    drop(store);
    let store = Store::open(path).unwrap();
    let applied: i64 = store
        .with(|c| c.query_row("SELECT COUNT(*) FROM schema_migrations", [], |r| r.get(0)))
        .unwrap();
    assert_eq!(applied as usize, MIGRATIONS.len());
    drop(store);
    std::fs::remove_dir_all(&dir).unwrap();
}
