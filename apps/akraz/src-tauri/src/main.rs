fn main() {
    if let Err(error) = akraz_app_lib::run() {
        eprintln!("error while running akraz: {error}");
        std::process::exit(1);
    }
}
