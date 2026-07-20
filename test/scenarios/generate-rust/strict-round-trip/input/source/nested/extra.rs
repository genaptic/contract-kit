impl crate::DeclarationOrder {
    pub fn total(&self) -> u32 {
        u32::from(self.zeta) + u32::from(self.alpha) + self.middle
    }
}

pub fn extra() -> bool {
    true
}

pub extern "Rust" fn explicit_rust() {}
