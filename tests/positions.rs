//! Error positions beyond the vectors (spec §8, *Error positions*): the
//! 0-based offset of the first character of the token at fault, which is a
//! code-point offset as well as a byte offset, since every character before it
//! is ASCII.

use dtrexp::parse;

fn err_at(expr: &str, pos: usize) {
    match parse(expr) {
        Ok(_) => panic!("`{expr}` should be rejected"),
        Err(e) => assert_eq!(e.pos, pos, "`{expr}` rejected at the wrong position: {e}"),
    }
}

#[test]
fn a_value_out_of_its_domain_is_named_by_the_value() {
    err_at("M1,3:13", 5);
    err_at("D5:40", 3);
    err_at("H0:20/4 M0:7", 9);
    err_at("M3:99/4", 3);
    err_at("E8#2", 1);
    err_at("E-8#2", 1);
    err_at("Y2000:10000/4/2", 6);
}

#[test]
fn a_count_that_breaks_its_rule_is_named_by_the_count() {
    err_at("E7#-6", 3);
    err_at("M1/3/0", 5);
    err_at("M3/1", 3);
    err_at("Y2000/4/4", 8);
    err_at("20200101/2D/0D", 12);
    err_at("20200101/1W/7D", 12);
}

#[test]
fn a_rule_between_two_tokens_names_the_later() {
    err_at("Y2020:2010", 6);
    err_at("Y2030:2020/2", 6);
    err_at("M5:3/2", 4);
    err_at("20180121T0000:20180120", 14);
}

#[test]
fn a_token_the_grammar_does_not_allow_is_named_by_itself() {
    err_at("*5", 1);
    err_at("E1:3#2", 4);
    err_at("E*:3#2", 4);
    err_at("M!1/2", 3);
    err_at("M!3,*", 4);
    err_at("M1,*:*,*", 7);
    err_at("T0900:1200,1200:1200", 16);
    err_at("T0900:1", 6);
    err_at("T0900:120000.5", 12);
}

#[test]
fn a_repeated_designator_fails_on_its_own_syntax_first() {
    err_at("M3 M", 4);
    err_at("M3 M13", 3);
}

#[test]
fn each_branch_is_checked_before_the_next_is_read() {
    err_at("M13 | M", 1);
    err_at("M3 | D40 | X", 6);
}

#[test]
fn a_non_ascii_character_is_named_at_its_code_point() {
    err_at("M1 D2 é", 6);
    err_at("M1 Y٢٠٢٠", 4);
}

#[test]
fn when_several_rules_fail_the_earliest_position_wins() {
    err_at("M11:2/1", 5); // the stride on a wrap, before its interval of 1
    err_at("M13/1", 1); // the value, before the interval
    err_at("M13/2/5", 1);
    err_at("E9#7", 1); // the weekday, before the ordinal
    err_at("M13 M3", 1); // the value, before the repeat
    err_at("M3 M13", 3); // the repeat, before the value
    err_at("D40/1 M1", 1);
    err_at("M-0/1", 1);
    err_at("M2147483648/1", 1);
    err_at("20230229/0D", 0);
    err_at("20200101:20190101 M13", 9);
    err_at("T250000.5", 1); // the hour, before the malformed fraction
    err_at("T0900:0900,25", 6);
    err_at("M*:5/1", 4);
    err_at("20200101/0D/1M", 9);
    err_at("20200101 20190101:20180101", 9);
    err_at("T0900 T0960", 6);
    err_at("20200101/1D 20200101/0D", 9);
}

#[test]
fn a_grammar_error_after_a_broken_rule_names_the_rule() {
    err_at("M1/1 X", 3);
    err_at("20230229 *", 0);
    err_at("E7#9 |", 3);
}

/// A branch that broke a rule is never evaluated for its warnings: a stride
/// of 0 would divide by zero, a year the rules reject would empty the year
/// span, and these all panicked before.
#[test]
fn a_rejected_value_never_reaches_the_warnings() {
    for (expr, pos) in [
        ("D1/0", 3),
        ("M1/0", 3),
        ("Y1:9999/0", 8),
        ("D1/00", 3),
        ("D366 Y-1", 6),
        ("W53 Y-1", 5),
        ("Y1001:2W53", 6),
        ("Y2020:2019 D366", 6),
        ("W53 Y-1:-1", 5),
    ] {
        err_at(expr, pos);
        assert_eq!(dtrexp::validate(expr).unwrap_err().pos, pos, "`{expr}`");
    }
}

#[test]
fn a_bare_star_in_a_list_is_decided_at_the_token_after_it() {
    err_at("M1,*,X", 3);
    err_at("M!*,X", 2);
    err_at("M!3,*,X", 4);
    // a rule error, so the waiting `D` check is earlier at the branch's end
    err_at("D40,*", 1);
}

#[test]
fn a_malformed_literal_time_value_or_fraction_is_a_rule_error() {
    err_at("D40 20180120T09", 1);
    err_at("D40 2020", 1);
    err_at("D40 201801201", 1);
    err_at("D40 T090", 1);
    err_at("D40 T1", 1);
    err_at("D40 T0900.5", 1);
    err_at("D40 T090000.5", 1);
    err_at("D40 T2400.000", 1);
    // each is named at its own token, read past whole
    err_at("T0900.5 X", 5);
    err_at("T090.500 X", 1);
    err_at("T0900:2400.000", 6);
    err_at("2020 X", 0);
    err_at("20200101T9 X", 0);
    err_at("20200101:2020 X", 9);
}

#[test]
fn a_value_is_decided_as_it_is_read_not_when_its_selector_ends() {
    err_at("M13,X", 1);
    err_at("E12,E", 1);
    err_at("H0/40/", 3);
    err_at("Y2020:2010,", 6);
    // a range of one year is no wrap
    assert!(parse("Y2020:2020").is_ok());
    err_at("M!0,", 2);
}

/// A long input that breaks a rule early is still read to its end, in time
/// linear in its length: each selector is checked once, never against the
/// whole branch before it.
#[test]
fn reading_on_after_an_error_is_linear() {
    let input = "D1 ".repeat(200_000);
    let start = std::time::Instant::now();
    err_at(&input, 3);
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs() < 5, "200,000 selectors took {elapsed:?}");
}

/// Only a `D` selector waits for its branch: its values and its stride
/// interval are checked against the scope once the branch is complete.
#[test]
fn a_d_selector_is_checked_when_its_branch_is_complete() {
    assert!(parse("D* M2").is_ok());
    assert!(parse("D*:5").is_ok());
    err_at("D*:32", 3);
    assert!(parse("D1:5 D*").is_err());
    err_at("D40:41", 1);
    err_at("D1:40/2", 3);
    err_at("D1/40", 3);
    err_at("D1/32 M1", 3);
    assert!(parse("D1/32 Y*").is_ok());
}

#[test]
fn a_missing_time_value_is_a_grammar_error() {
    err_at("D40 T,0900", 5);
    err_at("D40 T0900,", 10);
    err_at("D40 T0900:", 10);
}
