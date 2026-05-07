fn main() {
    if std::env::args().any(|a| a == "--help") {
        println!("stores - Schema-driven store framework");
    }
}
