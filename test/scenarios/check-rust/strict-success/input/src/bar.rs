use crate::foobar::FooBar;

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Foo {
    #[default]
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
        Self::default()
    }

    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Foo => "Foo".to_string(),
            Self::Bar(value) => format!("Bar({value})"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

#[cfg(test)]
mod tests {
    use super::{Bar, Foo};
    use crate::foobar::FooBar;

    #[test]
    fn foo_tracks_unit_and_tuple_variants() {
        let foo = Foo::new_foo();
        let bar = Foo::new_bar(5);

        assert!(foo.is_foo());
        assert!(!foo.is_bar());
        assert!(bar.is_bar());
        assert!(!bar.is_foo());
        assert_eq!(foo.value(), None);
        assert_eq!(bar.value(), Some(5));
        assert_eq!(foo.increment(), Foo::new_bar(1));
        assert_eq!(bar.increment(), Foo::new_bar(6));
        assert_eq!(bar.reset(), Foo::new_foo());
        assert_eq!(foo.describe(), "Foo");
        assert_eq!(bar.describe(), "Bar(5)");
    }

    #[test]
    fn bar_defaults_to_zero_tuple_variant() {
        let value = Bar::default();

        assert_eq!(value, Bar::new_bar(0));
        assert_eq!(value.value(), Some(0));
    }

    #[test]
    fn bar_implements_foobar_trait() {
        let foo = Bar::new_foo();
        let bar = Bar::new_bar(8);

        assert!(foo.is_foo());
        assert!(!foo.is_bar());
        assert_eq!(foo.describe(), "Foo");
        assert_eq!(foo.kind_label(), "foo");

        assert!(bar.is_bar());
        assert!(!bar.is_foo());
        assert_eq!(bar.value(), Some(8));
        assert_eq!(bar.describe(), "Bar(8)");
        assert_eq!(bar.kind_label(), "bar");
    }
}
