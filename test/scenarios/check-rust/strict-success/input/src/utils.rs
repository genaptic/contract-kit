use crate::bar::Foo;
use crate::foobar::FooBar;

#[derive(Clone, Debug, PartialEq, Eq)]
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

#[cfg(test)]
mod tests {
    use super::{describe_foobar, first_bar_value, parse_positive, sum_values, ParseError};
    use crate::bar::{Bar, Foo};

    #[test]
    fn sum_values_adds_only_tuple_values() {
        let values = [Foo::new_foo(), Foo::new_bar(2), Foo::new_bar(3)];

        assert_eq!(sum_values(&values), 5);
    }

    #[test]
    fn first_bar_value_returns_first_tuple_value() {
        let values = [Foo::new_foo(), Foo::new_bar(2), Foo::new_bar(3)];

        assert_eq!(first_bar_value(&values), Some(2));
        assert_eq!(first_bar_value(&[Foo::new_foo()]), None);
    }

    #[test]
    fn parse_positive_accepts_positive_integer_text() {
        assert_eq!(parse_positive(" 6 "), Ok(6));
    }

    #[test]
    fn parse_positive_rejects_empty_invalid_and_non_positive_input() {
        assert_eq!(parse_positive(""), Err(ParseError::Empty));
        assert_eq!(parse_positive("wat"), Err(ParseError::Invalid));
        assert_eq!(parse_positive("0"), Err(ParseError::NotPositive));
        assert_eq!(parse_positive("-1"), Err(ParseError::NotPositive));
    }

    #[test]
    fn describe_foobar_formats_trait_values() {
        let value = Bar::new_bar(4);

        assert_eq!(describe_foobar(&value), "bar: Bar(4)");
    }
}
