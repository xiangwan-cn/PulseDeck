pub fn number(value: f64, unit: Option<&str>, decimals: u8) -> String {
    if !value.is_finite() {
        return "—".into();
    }
    let value = format!("{:.*}", decimals as usize, value);
    let Some(unit) = unit.filter(|unit| !unit.is_empty()) else {
        return value;
    };
    let separator = if unit
        .chars()
        .next()
        .is_some_and(|first| first == '°' || first == '%' || first == '/' || is_cjk(first))
    {
        ""
    } else {
        " "
    };
    format!("{value}{separator}{unit}")
}

pub fn percentage(value: f64) -> String {
    if !value.is_finite() {
        return "—".into();
    }
    let value = value.clamp(0.0, 100.0);
    if (value - value.round()).abs() < 0.05 {
        format!("{value:.0}%")
    } else {
        format!("{value:.1}%")
    }
}

fn is_cjk(value: char) -> bool {
    matches!(
        value,
        '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{f900}'..='\u{faff}'
    )
}

#[cfg(test)]
mod tests {
    use super::{number, percentage};

    #[test]
    fn units_use_readable_spacing() {
        assert_eq!(number(42.0, Some("°C"), 1), "42.0°C");
        assert_eq!(number(123.0, Some("个"), 0), "123个");
        assert_eq!(number(4.2, Some("W"), 1), "4.2 W");
        assert_eq!(number(7.0, None, 0), "7");
    }

    #[test]
    fn whole_percentages_do_not_show_noise() {
        assert_eq!(percentage(72.0), "72%");
        assert_eq!(percentage(72.36), "72.4%");
    }
}
