fn main() {
    #[cfg(target_os = "windows")]
    std::thread::park();

    #[cfg(not(target_os = "windows"))]
    eprintln!("unsupported_platform");
}
