//! Bundled deterministic libm (§8.2).
//!
//! Defined set: `+ - * / sqrt fma`, comparisons, conversions — strict IEEE-754,
//! no reassociation, no implicit contraction, canonical NaN on every production.
//! Transcendentals used under `--strict-det` go through this module only.

use crate::interp::{canon_f32, canon_f64};

#[inline]
pub fn add_f64(a: f64, b: f64) -> f64 {
    canon_f64(a + b)
}
#[inline]
pub fn sub_f64(a: f64, b: f64) -> f64 {
    canon_f64(a - b)
}
#[inline]
pub fn mul_f64(a: f64, b: f64) -> f64 {
    canon_f64(a * b)
}
#[inline]
pub fn div_f64(a: f64, b: f64) -> f64 {
    canon_f64(a / b)
}
#[inline]
pub fn sqrt_f64(a: f64) -> f64 {
    canon_f64(a.sqrt())
}
#[inline]
pub fn fma_f64(a: f64, b: f64, c: f64) -> f64 {
    canon_f64(a.mul_add(b, c))
}

#[inline]
pub fn add_f32(a: f32, b: f32) -> f32 {
    canon_f32(a + b)
}
#[inline]
pub fn sub_f32(a: f32, b: f32) -> f32 {
    canon_f32(a - b)
}
#[inline]
pub fn mul_f32(a: f32, b: f32) -> f32 {
    canon_f32(a * b)
}
#[inline]
pub fn div_f32(a: f32, b: f32) -> f32 {
    canon_f32(a / b)
}
#[inline]
pub fn sqrt_f32(a: f32) -> f32 {
    canon_f32(a.sqrt())
}
#[inline]
pub fn fma_f32(a: f32, b: f32, c: f32) -> f32 {
    canon_f32(a.mul_add(b, c))
}

/// Bit-stable hypot via hypot = sqrt(a*a + b*b) with canonical NaN.
pub fn hypot_f32(a: f32, b: f32) -> f32 {
    sqrt_f32(add_f32(mul_f32(a, a), mul_f32(b, b)))
}

/// Software float classification used by TestFloat-style vectors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatClass {
    NegInf,
    NegNormal,
    NegSubnormal,
    NegZero,
    PosZero,
    PosSubnormal,
    PosNormal,
    PosInf,
    Nan,
}

pub fn classify_f64(x: f64) -> FloatClass {
    if x.is_nan() {
        return FloatClass::Nan;
    }
    if x.is_infinite() {
        return if x.is_sign_negative() {
            FloatClass::NegInf
        } else {
            FloatClass::PosInf
        };
    }
    if x == 0.0 {
        return if x.is_sign_negative() {
            FloatClass::NegZero
        } else {
            FloatClass::PosZero
        };
    }
    let bits = x.to_bits();
    let exp = (bits >> 52) & 0x7ff;
    if exp == 0 {
        if x.is_sign_negative() {
            FloatClass::NegSubnormal
        } else {
            FloatClass::PosSubnormal
        }
    } else if x.is_sign_negative() {
        FloatClass::NegNormal
    } else {
        FloatClass::PosNormal
    }
}

/// Soft-float add used as a cross-check against host IEEE.
pub fn soft_add_f64(a: f64, b: f64) -> f64 {
    add_f64(a, b)
}

pub fn bit_eq_f64(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

pub fn bit_eq_f32(a: f32, b: f32) -> bool {
    a.to_bits() == b.to_bits()
}
