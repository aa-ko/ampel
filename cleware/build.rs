//! Compile the vendor Cleware driver (`vendor/cleware/`, GPL-3.0) straight into
//! this crate. Two `cc` builds because C and C++ cannot share one invocation.

const VENDOR: &str = "../vendor/cleware";

fn main() {
    println!("cargo:rerun-if-changed=src/shim.cpp");
    for file in ["USBaccessBasic.c", "USBaccess.cpp", "USBswitchCmd.cpp"] {
        println!("cargo:rerun-if-changed={VENDOR}/{file}");
    }

    cc::Build::new()
        .file(format!("{VENDOR}/USBaccessBasic.c"))
        .define("CLEWARELINUX", None)
        .warnings(false)
        .compile("cleware_basic");

    cc::Build::new()
        .cpp(true)
        .file(format!("{VENDOR}/USBaccess.cpp"))
        .file(format!("{VENDOR}/USBswitchCmd.cpp"))
        .file("src/shim.cpp")
        .include(VENDOR)
        .define("CLEWARELINUX", None)
        // USBswitchCmd.cpp is the vendor CLI: keep its doSwitch(), rename the
        // main() that would clash with the Rust binary's own entry point. That
        // main() has no return type (pre-C89 style, which GCC tolerates only for
        // main itself), so the renamed copy needs -fpermissive.
        .define("main", "cleware_unused_cli_main")
        .flag("-fpermissive")
        .warnings(false)
        .compile("cleware_switch");
}
