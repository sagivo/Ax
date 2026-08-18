// Fixture for `ax harvest` (not a rustc checkout).
fn main() {
    let mut i = 0;
    let x = &mut i;
    let a = &mut i; //~ ERROR cannot borrow `i` as mutable more than once
    x;
}
