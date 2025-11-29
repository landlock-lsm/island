use std::process::Command;

#[test]
fn test_zsh_hook() {
    let status = Command::new("zsh")
        .arg("tests/shell/test_hook.zsh")
        .status()
        .expect("failed to execute zsh test script");

    assert!(status.success(), "Zsh hook tests failed");
}
