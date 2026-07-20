use crate::foobar::FooBar;

pub(crate) enum Foo {
    Foo,
    Bar(i32),
}

impl Foo {
    pub(crate) fn new_foo() -> Self {
        Self::Foo
    }

    pub(crate) fn new_bar(value: i32) -> Self {
        Self::Bar(value)
    }

    pub(crate) fn is_foo(&self) -> bool {
        matches!(self, Self::Foo)
    }

    pub(crate) fn is_bar(&self) -> bool {
        matches!(self, Self::Bar(_))
    }

    pub(crate) fn value(&self) -> Option<i32> {
        match self {
            Self::Foo => None,
            Self::Bar(value) => Some(*value),
        }
    }

    pub(crate) fn increment(&self) -> Self {
        match self {
            Self::Foo => Self::Bar(1),
            Self::Bar(value) => Self::Bar(value + 1),
        }
    }

    pub(crate) fn reset(&self) -> Self {
        Self::Foo
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Foo => "Foo".to_string(),
            Self::Bar(value) => format!("Bar({value})"),
        }
    }
}

pub enum Bar {
    Foo,
    Bar(i32),
}

impl Bar {
    pub(crate) fn new_foo() -> Self {
        Self::Foo
    }

    pub(crate) fn new_bar(value: i32) -> Self {
        Self::Bar(value)
    }

    pub(crate) fn value(&self) -> Option<i32> {
        match self {
            Self::Foo => None,
            Self::Bar(value) => Some(*value),
        }
    }
}

impl Default for Bar {
    fn default() -> Self {
        Self::new_bar(0)
    }
}

impl FooBar for Bar {
    fn is_foo(&self) -> bool {
        matches!(self, Self::Foo)
    }

    fn is_bar(&self) -> bool {
        matches!(self, Self::Bar(_))
    }

    fn describe(&self) -> String {
        match self {
            Self::Foo => "Foo".to_string(),
            Self::Bar(value) => format!("Bar({value})"),
        }
    }
}
