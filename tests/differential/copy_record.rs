#[derive(Copy, Clone)]
struct R { s: i32 }
fn main() {
    let mut x = R { s: 5 };
    let y = x;
    x.s = 6;
    print!("{}i32", y.s + x.s);
}
