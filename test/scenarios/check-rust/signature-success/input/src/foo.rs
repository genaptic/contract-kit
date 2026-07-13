use crate::bar::Foo;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FooStruct {
    foo: Foo,
    int_field: i32,
}

impl FooStruct {
    pub(crate) fn new(foo: Foo, int_field: i32) -> Self {
        Self { foo, int_field }
    }

    pub(crate) fn foo(&self) -> &Foo {
        &self.foo
    }

    pub(crate) fn int_field(&self) -> i32 {
        self.int_field
    }

    pub(crate) fn total(&self) -> i32 {
        self.foo.value().unwrap_or_default() + self.int_field
    }

    pub(crate) fn increment_foo(&mut self) {
        self.foo = self.foo.increment();
    }

    pub(crate) fn describe(&self) -> String {
        format!(
            "FooStruct(foo={}, int_field={})",
            self.foo.describe(),
            self.int_field
        )
    }
}

#[cfg(test)]
mod tests {
    use super::FooStruct;
    use crate::bar::Foo;

    #[test]
    fn foo_struct_exposes_fields_through_accessors() {
        let value = FooStruct::new(Foo::new_bar(10), 2);

        assert_eq!(value.foo(), &Foo::new_bar(10));
        assert_eq!(value.int_field(), 2);
        assert_eq!(value.total(), 12);
    }

    #[test]
    fn foo_struct_can_increment_inner_foo() {
        let mut value = FooStruct::new(Foo::new_bar(10), 2);

        value.increment_foo();

        assert_eq!(value.foo(), &Foo::new_bar(11));
        assert_eq!(value.total(), 13);
    }

    #[test]
    fn foo_struct_describes_its_fields() {
        let value = FooStruct::new(Foo::new_foo(), 3);

        assert_eq!(value.describe(), "FooStruct(foo=Foo, int_field=3)");
    }
}
