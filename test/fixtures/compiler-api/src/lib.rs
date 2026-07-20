#![allow(dead_code)]

macro_rules! exported_item {
    ($name:ident) => {
        pub struct $name;
    };
}

exported_item!(MacroGenerated);

pub mod public_api {
    pub struct PublicModuleItem;
}

mod direct {
    pub struct DirectTarget;
}

mod globbed {
    pub struct GlobTarget;

    pub mod nested {
        pub struct NestedTarget;
    }

    pub use nested::*;
}

pub use direct::DirectTarget as RenamedTarget;
pub use globbed::*;

#[cfg(feature = "selected")]
pub struct SelectedApi;

#[cfg(not(feature = "selected"))]
pub struct DisabledApi;

pub type GenericAlias<'a, T = u8, const N: usize = 4> = &'a [T; N];

pub fn normalized_alias(value: GenericAlias<'static>) -> &'static [u8; 4] {
    value
}

pub trait ExtractedTrait {
    type Output;
    const ENABLED: bool;

    fn value(&self) -> Self::Output;
}

pub struct ImplementedApi;

impl ImplementedApi {
    pub const ID: u8 = 7;

    pub fn new() -> Self {
        Self
    }
}

impl ExtractedTrait for ImplementedApi {
    type Output = u8;
    const ENABLED: bool = true;

    fn value(&self) -> Self::Output {
        Self::ID
    }
}

#[cfg(feature = "broken")]
compile_error!("intentionally noncompiling compiler fixture");
