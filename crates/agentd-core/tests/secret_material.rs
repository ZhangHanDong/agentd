use agentd_core::ports::SecretMaterial;

#[test]
fn secret_material_debug_is_redacted() {
    let secret = SecretMaterial::new("top-secret-value");
    let rendered = format!("{secret:?}");
    assert!(!rendered.contains("top-secret-value"));
    assert!(rendered.contains("REDACTED"));
}
