use crate::bar::Foo;
use crate::foobar::FooBar;

pub(crate) enum ParseError {
    Empty,
    Invalid,
    NotPositive,
}

pub(crate) fn sum_values(values: &[Foo]) -> i32 {
    values.iter().filter_map(Foo::value).sum()
}

pub(crate) fn first_bar_value(values: &[Foo]) -> Option<i32> {
    values.iter().find_map(Foo::value)
}

pub(crate) fn parse_positive(input: &str) -> Result<i32, ParseError> {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return Err(ParseError::Empty);
    }

    let value = trimmed.parse::<i32>().map_err(|_| ParseError::Invalid)?;

    if value <= 0 {
        return Err(ParseError::NotPositive);
    }

    Ok(value)
}

pub(crate) fn describe_foobar<T: FooBar>(value: &T) -> String {
    format!("{}: {}", value.kind_label(), value.describe())
}
