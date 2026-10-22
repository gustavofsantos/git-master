mod battery;
mod sandbox;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let want = |s: &str| args.is_empty() || args.iter().any(|a| a == s);
    if args.iter().any(|a| a == "--globals") {
        println!("{:?}", battery::lua_stdlib_check());
        return;
    }
    if want("A") {
        battery::section_a();
    }
    if want("F") {
        battery::section_f();
    }
    if want("B") {
        battery::section_b();
    }
    if want("C") {
        battery::section_c();
    }
    if want("D") {
        battery::section_d();
    }
    if want("N") || args.is_empty() {
        battery::section_f_naive();
    }
    if want("V") || args.is_empty() {
        battery::section_f_no_switch();
    }
    if want("P") || args.is_empty() {
        battery::section_f_pitfall();
    }
}
