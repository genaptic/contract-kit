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
