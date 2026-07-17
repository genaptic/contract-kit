use crate::bar::Foo;

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
