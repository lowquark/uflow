
pub type Type = u32;

pub const MASK: Type = 0xFFFFF;
pub const SPAN: Type = 0x100000;

pub fn add(a: Type, b: Type) -> Type {
    a.wrapping_add(b) & MASK
}

pub fn sub(a: Type, b: Type) -> Type {
    a.wrapping_sub(b) & MASK
}

pub fn is_valid(a: Type) -> bool {
    a & MASK == a
}

