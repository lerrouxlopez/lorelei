use jsonschema::Validator;
use lorelei_core::ShellRisk;
use lorelei_shells::ShellRegistryPg;
use sqlx::PgPool;

#[tokio::test]
async fn invalid_shell_name_rejected() {
    let pool = PgPool::connect_lazy("postgres://localhost/does_not_matter").unwrap();
    let reg = ShellRegistryPg::new(pool);
    assert!(reg.validate_name("does_not_exist").is_err());
}

#[tokio::test]
async fn schema_validation_catches_missing_required_field() {
    let pool = PgPool::connect_lazy("postgres://localhost/does_not_matter").unwrap();
    let reg = ShellRegistryPg::new(pool);

    let schema = reg.schema("echo").unwrap();
    let v = Validator::new(&schema).unwrap();
    let bad = serde_json::json!({});
    assert!(v.is_valid(&bad) == false);
}

#[tokio::test]
async fn risk_levels_match_expectations() {
    let pool = PgPool::connect_lazy("postgres://localhost/does_not_matter").unwrap();
    let reg = ShellRegistryPg::new(pool);

    assert_eq!(reg.risk("noop").unwrap(), ShellRisk::Low);
    assert_eq!(reg.risk("echo").unwrap(), ShellRisk::Low);
    assert_eq!(reg.risk("forget_pearl").unwrap(), ShellRisk::High);
}

