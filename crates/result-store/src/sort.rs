use std::cmp::Ordering;

/// 比較方法。DB の列型から呼び出し側が選ぶ。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKind {
    /// UTF-8 のバイト列として比較する(ロケールの照合順序ではない)。
    /// ISO 形式の日時はこれで時系列順になる。
    Text,
    /// 10 進数の文字列として比較する。f64 には変換せず、桁数に上限がない。
    Numeric,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

/// 並べ替え済みの表示順。元データは並べ替えない。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SortIndex {
    order: Vec<u32>,
}

impl SortIndex {
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// 表示上の `position` 番目の行の、元の行番号。
    pub fn source_row(&self, position: u64) -> Option<u64> {
        self.order
            .get(usize::try_from(position).ok()?)
            .map(|&r| r as u64)
    }
}

/// 並べ替える 1 列の値だけを集めたもの。全列を複製せずに済む。
pub(crate) struct ColumnKeys {
    data: Vec<u8>,
    /// `(start, end)`。NULL は `start == NULL_MARK`
    ranges: Vec<(u32, u32)>,
}

const NULL_MARK: u32 = u32::MAX;

impl ColumnKeys {
    pub fn with_capacity(rows: usize) -> Self {
        Self {
            data: Vec::new(),
            ranges: Vec::with_capacity(rows),
        }
    }

    /// 値の合計が 4 GiB に収まらなければ false。
    pub fn push(&mut self, cell: Option<&[u8]>) -> bool {
        match cell {
            None => self.ranges.push((NULL_MARK, NULL_MARK)),
            Some(b) => {
                let start = self.data.len();
                if start + b.len() >= NULL_MARK as usize {
                    return false;
                }
                self.data.extend_from_slice(b);
                self.ranges.push((start as u32, self.data.len() as u32));
            }
        }
        true
    }

    fn get(&self, i: usize) -> Option<&[u8]> {
        let (s, e) = self.ranges[i];
        (s != NULL_MARK).then(|| &self.data[s as usize..e as usize])
    }

    pub fn sort(self, order: SortOrder, kind: SortKind) -> SortIndex {
        let n = self.ranges.len();
        let mut idx: Vec<u32> = (0..n as u32).collect();
        match kind {
            SortKind::Text => {
                idx.sort_by(|&a, &b| {
                    nulls_largest(
                        self.get(a as usize),
                        self.get(b as usize),
                        |x, y| x.cmp(y),
                        order,
                    )
                });
            }
            SortKind::Numeric => {
                // 桁は行ごとに確保せず、1 つの領域に連結して持つ
                let mut pool = DigitPool::default();
                let nums: Vec<Option<Num>> = (0..n)
                    .map(|i| self.get(i).map(|b| Num::parse(b, &mut pool)))
                    .collect();
                // 解析済みなので、元の文字列は手放す(比較に使うのは桁の領域だけ)
                drop(self);
                idx.sort_by(|&a, &b| {
                    nulls_largest(
                        nums[a as usize].as_ref(),
                        nums[b as usize].as_ref(),
                        |x, y| x.cmp(y, &pool),
                        order,
                    )
                });
            }
        }
        SortIndex { order: idx }
    }
}

/// PostgreSQL と同じく NULL を最大として扱う(昇順で最後、降順で最初)。
/// 同値は元の行番号順を保つ(安定ソート)。
fn nulls_largest<T>(
    a: Option<T>,
    b: Option<T>,
    cmp: impl Fn(&T, &T) -> Ordering,
    order: SortOrder,
) -> Ordering {
    let asc = match (&a, &b) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => cmp(x, y),
    };
    match order {
        SortOrder::Ascending => asc,
        SortOrder::Descending => asc.reverse(),
    }
}

// ---- 10 進数の比較 ----

/// 解析した数値。値は `0.d1d2d3… × 10^exp`(d1 ≠ 0)で、桁は `DigitPool` に置く。
#[derive(Clone, Copy, Debug)]
pub(crate) struct Num {
    class: NumClass,
    negative: bool,
    exp: i64,
    digits: (u32, u32),
}

/// 小さいほど先に並ぶ。数値として読めない値は NaN の後ろ。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum NumClass {
    NegInfinity,
    Finite,
    PosInfinity,
    NaN,
    Invalid,
}

/// 全行の有効桁を連結した領域。`Num::digits` はこの中の範囲を指す。
#[derive(Default)]
struct DigitPool {
    digits: Vec<u8>,
}

impl Num {
    fn parse(s: &[u8], pool: &mut DigitPool) -> Num {
        let invalid = Num {
            class: NumClass::Invalid,
            negative: false,
            exp: 0,
            digits: (0, 0),
        };
        let Ok(s) = std::str::from_utf8(s) else {
            return invalid;
        };
        let s = s.trim();
        let (negative, body) = match s.as_bytes().first() {
            Some(b'-') => (true, &s[1..]),
            Some(b'+') => (false, &s[1..]),
            _ => (false, s),
        };
        let special = |class| Num {
            class,
            negative,
            exp: 0,
            digits: (0, 0),
        };
        if body.eq_ignore_ascii_case("nan") {
            return special(NumClass::NaN);
        }
        if body.eq_ignore_ascii_case("infinity") || body.eq_ignore_ascii_case("inf") {
            return special(if negative {
                NumClass::NegInfinity
            } else {
                NumClass::PosInfinity
            });
        }

        let (mantissa, exp10) = match body.find(['e', 'E']) {
            Some(i) => match body[i + 1..].parse::<i64>() {
                Ok(e) => (&body[..i], e),
                Err(_) => return invalid,
            },
            None => (body, 0),
        };
        let (int_part, frac_part) = match mantissa.split_once('.') {
            Some((i, f)) => (i, f),
            None => (mantissa, ""),
        };
        let all_digits = |p: &str| p.bytes().all(|b| b.is_ascii_digit());
        if (int_part.is_empty() && frac_part.is_empty())
            || !all_digits(int_part)
            || !all_digits(frac_part)
        {
            return invalid;
        }

        // 先頭の 0 を除く。除いた分だけ小数点の位置が前に動く
        let joined = int_part.bytes().chain(frac_part.bytes());
        let leading_zeros = joined.clone().take_while(|&b| b == b'0').count();
        let all_len = int_part.len() + frac_part.len();
        if leading_zeros == all_len {
            return Num {
                class: NumClass::Finite,
                negative: false, // -0 と 0 は等しい
                exp: 0,
                digits: (0, 0),
            };
        }
        let exp = int_part.len() as i64 - leading_zeros as i64 + exp10;
        let start = pool.digits.len();
        pool.digits.extend(joined.skip(leading_zeros));
        while pool.digits.len() > start && pool.digits.last() == Some(&b'0') {
            pool.digits.pop();
        }
        Num {
            class: NumClass::Finite,
            negative,
            exp,
            digits: (start as u32, pool.digits.len() as u32),
        }
    }

    fn is_zero(&self) -> bool {
        self.class == NumClass::Finite && self.digits.0 == self.digits.1
    }

    fn cmp(&self, other: &Num, pool: &DigitPool) -> Ordering {
        if self.class != NumClass::Finite || other.class != NumClass::Finite {
            return self.class.cmp(&other.class);
        }
        let sign = |n: &Num| {
            if n.is_zero() {
                0
            } else if n.negative {
                -1
            } else {
                1
            }
        };
        let (sa, sb) = (sign(self), sign(other));
        if sa != sb {
            return sa.cmp(&sb);
        }
        if sa == 0 {
            return Ordering::Equal;
        }
        let digits = |n: &Num| &pool.digits[n.digits.0 as usize..n.digits.1 as usize];
        let magnitude = self
            .exp
            .cmp(&other.exp)
            .then_with(|| digits(self).cmp(digits(other)));
        if sa < 0 {
            magnitude.reverse()
        } else {
            magnitude
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sorted(values: &[Option<&str>], order: SortOrder, kind: SortKind) -> Vec<Option<String>> {
        let mut keys = ColumnKeys::with_capacity(values.len());
        for v in values {
            assert!(keys.push(v.map(str::as_bytes)));
        }
        let idx = keys.sort(order, kind);
        (0..values.len() as u64)
            .map(|p| values[idx.source_row(p).unwrap() as usize].map(str::to_string))
            .collect()
    }

    fn strs(v: &[&str]) -> Vec<Option<String>> {
        v.iter().map(|s| Some(s.to_string())).collect()
    }

    #[test]
    fn numeric_order_is_not_text_order() {
        let v = ["10", "9", "100", "-5", "0", "2.5"].map(Some);
        assert_eq!(
            sorted(&v, SortOrder::Ascending, SortKind::Numeric),
            strs(&["-5", "0", "2.5", "9", "10", "100"])
        );
        assert_eq!(
            sorted(&v, SortOrder::Ascending, SortKind::Text),
            strs(&["-5", "0", "10", "100", "2.5", "9"])
        );
    }

    #[test]
    fn numeric_keeps_precision_beyond_f64() {
        // f64 では同じ値になる 2 つ
        let a = "12345678901234567890.123456789012345678";
        let b = "12345678901234567890.123456789012345679";
        let v = [Some(b), Some(a)];
        assert_eq!(
            sorted(&v, SortOrder::Ascending, SortKind::Numeric),
            strs(&[a, b])
        );
        assert_eq!(a.parse::<f64>().unwrap(), b.parse::<f64>().unwrap());
    }

    #[test]
    fn numeric_handles_signs_zeros_and_forms() {
        let v = [
            "-0.50", "-0.5", "0.0", "-0", "+1", "1.0", "00012", "1e2", "0.001", "-1e-3",
        ]
        .map(Some);
        let got = sorted(&v, SortOrder::Ascending, SortKind::Numeric);
        assert_eq!(
            got,
            strs(&[
                "-0.50", "-0.5", "-1e-3", "0.0", "-0", "0.001", "+1", "1.0", "00012", "1e2"
            ])
        );
    }

    #[test]
    fn numeric_specials_follow_postgres_order() {
        let v = ["NaN", "Infinity", "5", "-Infinity", "abc", "-5"].map(Some);
        assert_eq!(
            sorted(&v, SortOrder::Ascending, SortKind::Numeric),
            strs(&["-Infinity", "-5", "5", "Infinity", "NaN", "abc"])
        );
    }

    #[test]
    fn nulls_sort_as_largest_and_ties_are_stable() {
        let v = [Some("b"), None, Some("a"), None, Some("a")];
        assert_eq!(
            sorted(&v, SortOrder::Ascending, SortKind::Text),
            vec![
                Some("a".into()),
                Some("a".into()),
                Some("b".into()),
                None,
                None
            ]
        );
        assert_eq!(
            sorted(&v, SortOrder::Descending, SortKind::Text),
            vec![
                None,
                None,
                Some("b".into()),
                Some("a".into()),
                Some("a".into())
            ]
        );
        let mut keys = ColumnKeys::with_capacity(3);
        for s in ["x", "x", "x"] {
            keys.push(Some(s.as_bytes()));
        }
        let idx = keys.sort(SortOrder::Ascending, SortKind::Text);
        assert_eq!(
            (0..3)
                .map(|p| idx.source_row(p).unwrap())
                .collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    #[test]
    fn empty_string_sorts_before_text_and_apart_from_null() {
        let v = [None, Some("a"), Some("")];
        assert_eq!(
            sorted(&v, SortOrder::Ascending, SortKind::Text),
            vec![Some("".into()), Some("a".into()), None]
        );
    }
}

#[cfg(test)]
mod reference_tests {
    use super::*;
    use proptest::prelude::*;

    /// 整数 15 桁・小数 10 桁までの 10 進数を、10^10 倍した i128 で表す(基準実装)。
    fn scaled(s: &str) -> i128 {
        let (neg, body) = match s.strip_prefix('-') {
            Some(b) => (true, b),
            None => (false, s),
        };
        let (int, frac) = body.split_once('.').unwrap_or((body, ""));
        let mut frac = frac.to_string();
        while frac.len() < 10 {
            frac.push('0');
        }
        let v: i128 = format!("{int}{frac}").parse().unwrap();
        if neg { -v } else { v }
    }

    fn compare(a: &str, b: &str) -> Ordering {
        let mut pool = DigitPool::default();
        let na = Num::parse(a.as_bytes(), &mut pool);
        let nb = Num::parse(b.as_bytes(), &mut pool);
        na.cmp(&nb, &pool)
    }

    proptest! {
        #[test]
        fn numeric_compare_matches_scaled_integer_reference(
            a in "-?[0-9]{1,15}(\\.[0-9]{1,10})?",
            b in "-?[0-9]{1,15}(\\.[0-9]{1,10})?",
        ) {
            prop_assert_eq!(compare(&a, &b), scaled(&a).cmp(&scaled(&b)), "{} vs {}", a, b);
        }

        #[test]
        fn numeric_compare_is_antisymmetric_and_never_panics(a in "\\PC{0,24}", b in "\\PC{0,24}") {
            prop_assert_eq!(compare(&a, &b), compare(&b, &a).reverse());
        }
    }
}
