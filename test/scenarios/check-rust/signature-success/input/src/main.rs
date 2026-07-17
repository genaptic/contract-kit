mod bar;
mod foo;
mod foobar;
mod utils;

use bar::Bar;
use bar::Foo;
use foo::FooStruct;
use foobar::FooBar;

fn main() {
    let foo = Foo::new_foo();
    let bar_value = Foo::new_bar(41).increment();
    let wrapped = FooStruct::new(bar_value, 1);

    assert!(foo.is_foo());
    assert!(!foo.is_bar());
    assert!(foo.reset().is_foo());
    assert_eq!(wrapped.total(), 43);
    assert_eq!(wrapped.foo().value(), Some(42));
    assert_eq!(wrapped.int_field(), 1);
    assert_eq!(wrapped.describe(), "FooStruct(foo=Bar(42), int_field=1)");

    let mut mutable_wrapped = FooStruct::new(Foo::new_bar(1), 2);
    mutable_wrapped.increment_foo();
    assert_eq!(mutable_wrapped.total(), 4);

    let trait_foo = Bar::new_foo();
    let trait_bar = Bar::new_bar(7);
    assert!(trait_foo.is_foo());
    assert_eq!(trait_foo.kind_label(), "foo");
    assert!(trait_bar.is_bar());
    assert_eq!(trait_bar.value(), Some(7));
    assert_eq!(trait_bar.describe(), "Bar(7)");
    assert_eq!(trait_bar.kind_label(), "bar");

    let values = [Foo::new_foo(), Foo::new_bar(3), Foo::new_bar(4)];
    assert_eq!(utils::sum_values(&values), 7);
    assert_eq!(utils::first_bar_value(&values), Some(3));
    assert!(matches!(utils::parse_positive("9"), Ok(9)));
    assert_eq!(utils::describe_foobar(&trait_bar), "bar: Bar(7)");
}
