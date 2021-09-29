
pub type Id = u32;

/*
pub fn lead_signed(a: Id, b: Id) -> i32 {
    let mut result = ((a.wrapping_sub(b)) & 0xFFFFFF) as i32;
    if result >= 0x800000 {
        result -= 0x1000000;
    }
    result
}
*/

pub fn lead_unsigned(a: Id, b: Id) -> Id {
    (a.wrapping_sub(b)) & 0xFFFFFF
}

pub fn add(a: Id, b: Id) -> Id {
    (a.wrapping_add(b)) & 0xFFFFFF
}

