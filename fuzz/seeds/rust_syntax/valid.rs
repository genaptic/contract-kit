#[repr(C)]
pub struct FuzzRecord {
    pub value: u32,
}

pub trait FuzzTrait {
    type Output;
    const ENABLED: bool;
    fn run(&self, value: FuzzRecord) -> Self::Output;
}
