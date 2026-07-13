pub trait FooBar {
    fn is_foo(&self) -> bool;

    fn is_bar(&self) -> bool;

    fn describe(&self) -> String;

    fn kind_label(&self) -> &'static str {
        if self.is_foo() {
            "foo"
        } else if self.is_bar() {
            "bar"
        } else {
            "unknown"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FooBar;

    struct TestFooBar {
        foo: bool,
        bar: bool,
    }

    impl TestFooBar {
        fn new(foo: bool, bar: bool) -> Self {
            Self { foo, bar }
        }
    }

    impl FooBar for TestFooBar {
        fn is_foo(&self) -> bool {
            self.foo
        }

        fn is_bar(&self) -> bool {
            self.bar
        }

        fn describe(&self) -> String {
            "test".to_string()
        }
    }

    #[test]
    fn kind_label_prefers_foo_when_present() {
        let value = TestFooBar::new(true, true);

        assert_eq!(value.kind_label(), "foo");
    }

    #[test]
    fn kind_label_returns_bar_for_bar_values() {
        let value = TestFooBar::new(false, true);

        assert_eq!(value.kind_label(), "bar");
    }

    #[test]
    fn kind_label_returns_unknown_without_known_state() {
        let value = TestFooBar::new(false, false);

        assert_eq!(value.kind_label(), "unknown");
    }
}
