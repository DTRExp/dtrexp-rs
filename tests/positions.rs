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
