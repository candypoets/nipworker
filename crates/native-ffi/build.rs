use std::env;
use std::path::PathBuf;

fn main() {
    let target = env::var("TARGET").unwrap_or_default();

    if target.contains("android") {
        let out_dir = env::var("OUT_DIR").unwrap();
        let out_path = PathBuf::from(&out_dir);

        // Compile the JNI C implementation to an object file.
        cc::Build::new()
            .file("android/nipworker_jni_impl.c")
            .cargo_metadata(false)
            .compile("nipworker_jni_impl");

        // Find the generated object file.
        let obj_file = std::fs::read_dir(&out_path)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.ends_with("-nipworker_jni_impl.o")
            })
            .map(|e| e.path())
            .next()
            .expect("nipworker_jni_impl object file not found");

        // Pass the object file directly to the linker.
        println!("cargo:rustc-link-arg={}", obj_file.display());

        // Link against the Android log library.
        println!("cargo:rustc-link-lib=log");
    }

    println!("cargo:rerun-if-changed=android/nipworker_jni_impl.c");
}
