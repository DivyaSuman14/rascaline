use std::path::PathBuf;
use std::process::Command;

mod utils;

#[test]
fn check_c_api() {
    const CARGO_TARGET_TMPDIR: &str = env!("CARGO_TARGET_TMPDIR");
    let mut build_dir = PathBuf::from(CARGO_TARGET_TMPDIR);
    build_dir.push("c-api-tests");
    std::fs::create_dir_all(&build_dir).expect("failed to create build dir");

    let mut source_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    source_dir.extend(["tests", "c-api"]);

    // assume that debug assertion means that we are building the code in
    // debug mode, even if that could be not true in some cases
    let build_type = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };

    let mut cmake_config = utils::cmake_config(&source_dir, &build_dir, build_type);
    cmake_config.arg("-DCMAKE_EXPORT_COMPILE_COMMANDS=ON");
    cmake_config.arg("-DRASCALINE_FETCH_EQUISTORE=ON");

    let status = cmake_config.status().expect("cmake configuration failed");
    assert!(status.success());

    let mut cmake_build = utils::cmake_build(&build_dir, build_type);
    let status = cmake_build.status().expect("cmake build failed");
    assert!(status.success());

    let ctest = which::which("ctest").expect("could not find ctest");
    let mut ctest = Command::new(ctest);
    ctest.current_dir(&build_dir);
    ctest.arg("--output-on-failure");
    ctest.arg("--C");
    ctest.arg(build_type);

    // std::env::var("LLVM_PROFILE_FILE")


    let status = ctest.status().expect("failed to run tests");
    assert!(status.success());
}
