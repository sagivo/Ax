struct P { x: i32, y: i32 }
fn main() {
    let p = P { x: 4, y: 9 };
    print!("{}i32", p.x + p.y);
}
