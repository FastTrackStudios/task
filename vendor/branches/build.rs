use rustc_version::{Version, version_meta};

// PATCHED (task, vendor/branches/PATCHED.md): always the stable code
// path. Upstream picks `branches_nightly` on a nightly rustc, which calls
// `core::intrinsics::abort` — gone from current nightlies — and the
// workspace builds on a dated nightly for the parallel front end.
fn main() {
    println!("cargo:rustc-cfg=branches_stable");
    if let Ok(meta) = version_meta() {
        if meta.semver >= Version::parse("1.81.0").unwrap() {
            println!("cargo:rustc-cfg=rustc_ge_1_81_0");
        }
        if meta.semver >= Version::parse("1.84.0").unwrap() {
            println!("cargo:rustc-cfg=rustc_ge_1_84_0");
        }
        if meta.semver >= Version::parse("1.95.0").unwrap() {
            println!("cargo:rustc-cfg=rustc_ge_1_95_0");
        }
    }
}
