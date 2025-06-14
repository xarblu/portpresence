use std::process::Command;

/// get the string returned by `ebuild --version`
/// (e.g. "Portage 3.0.68")
/// if anything goes wrong None is returned
pub(crate) fn ebuild_version() -> Result<String, String> {
    let ps = Command::new("ebuild")
        .args(["--version"])
        .output()
        .map_err(|e| e.to_string())?;

    if !ps.status.success() {
        return Err(format!("ebuild --version exited with {}", ps.status));
    }

    let stdout = String::from_utf8(ps.stdout).map_err(|e| e.to_string())?;
    let mut lines = stdout.lines();

    match lines.next() {
        Some(line) => Ok(String::from(line)),
        None => Err(String::from("Could not get 1st line from ebuild --version")),
    }
}
