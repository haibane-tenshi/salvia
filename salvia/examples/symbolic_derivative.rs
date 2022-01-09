//! Toy symbolic derivation.
//!
//! Features extensive use of recursive functions.

use async_recursion::async_recursion;
use salvia::{query, Input, InputAccess, QueryContext, Runtime};
use std::fmt::Display;
use std::str::FromStr;

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
struct ParseSymbolError;

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
enum Symbol {
    Const(i64),
    Var,
    Add(Box<Self>, Box<Self>),
    Mul(Box<Self>, Box<Self>),
    Pow(Box<Self>, u32),
}

impl Symbol {
    #[query]
    #[async_recursion]
    pub async fn derivative(self, cx: &QueryContext) -> Self {
        use tokio::join;
        use Symbol::*;

        match self {
            Const(_) => Const(0),
            Var => Const(1),
            Add(left, right) => {
                let (left, right) = join!(left.derivative(cx), right.derivative(cx));

                let r = Add(Box::new(left), Box::new(right));

                r.simplify(cx).await
            }
            Mul(left, right) => {
                let (dleft, dright) =
                    join!(left.clone().derivative(cx), right.clone().derivative(cx));

                let left = Box::new(Mul(left, Box::new(dright)));
                let right = Box::new(Mul(Box::new(dleft), right));

                let r = Add(left, right);

                r.simplify(cx).await
            }
            Pow(base, pow) => {
                let dbase = Box::new(base.clone().derivative(cx).await);
                let base = Box::new(Pow(base, pow - 1));
                let part = Box::new(Mul(base, dbase));

                let r = Mul(Box::new(Const(pow as i64)), part);

                r.simplify(cx).await
            }
        }
    }

    #[query]
    #[async_recursion]
    pub async fn eval(self, x: i64, cx: &QueryContext) -> i64 {
        use tokio::join;

        match self {
            Symbol::Const(n) => n,
            Symbol::Var => x,
            Symbol::Add(left, right) => {
                let (left, right) = join!(left.eval(x, cx), right.eval(x, cx));

                left + right
            }
            Symbol::Mul(left, right) => {
                let (left, right) = join!(left.eval(x, cx), right.eval(x, cx));

                left * right
            }
            Symbol::Pow(base, pow) => {
                let base = base.eval(x, cx).await;

                base.pow(pow)
            }
        }
    }

    #[query]
    #[async_recursion]
    pub async fn eval_const(self, cx: &QueryContext) -> Option<i64> {
        use tokio::join;
        use Symbol::*;

        let r = match self {
            Const(n) => n,
            Var => return None,
            Add(left, right) => {
                let (left, right) = join!(left.eval_const(cx), right.eval_const(cx));
                let left = left?;
                let right = right?;

                left + right
            }
            Mul(left, right) => {
                let (left, right) = join!(left.eval_const(cx), right.eval_const(cx));

                match (left, right) {
                    (Some(n), _) | (_, Some(n)) if n == 0 => 0,
                    (Some(n), Some(m)) => n * m,
                    _ => return None,
                }
            }
            Pow(base, pow) => {
                if pow == 0 {
                    1
                } else {
                    let base = base.eval_const(cx).await?;

                    base.pow(pow)
                }
            }
        };

        Some(r)
    }

    #[query]
    #[async_recursion]
    pub async fn simplify(self, cx: &QueryContext) -> Self {
        use tokio::join;
        use Symbol::*;

        match self {
            s @ (Const(_) | Var) => s,
            Add(left, right) => {
                let (left_const, right_const) =
                    join!(left.clone().eval_const(cx), right.clone().eval_const(cx));

                match ((left_const, left), (right_const, right)) {
                    ((Some(n), _), (Some(m), _)) => Const(n + m),
                    ((Some(n), _), (None, s)) | ((None, s), (Some(n), _)) => {
                        let s = s.simplify(cx).await;

                        match n {
                            0 => s,
                            n => Add(Box::new(s), Box::new(Const(n))),
                        }
                    }
                    ((_, left), (_, right)) => {
                        let (left, right) = join!(left.simplify(cx), right.simplify(cx));

                        Add(Box::new(left), Box::new(right))
                    }
                }
            }
            Mul(left, right) => {
                let (left_const, right_const) =
                    join!(left.clone().eval_const(cx), right.clone().eval_const(cx));

                match ((left_const, left), (right_const, right)) {
                    ((Some(n), _), (Some(m), _)) => Const(n * m),
                    ((Some(n), _), (None, s)) | ((None, s), (Some(n), _)) => {
                        let s = s.simplify(cx).await;

                        match n {
                            0 => Const(0),
                            1 => s,
                            n => Mul(Box::new(Const(n)), Box::new(s)),
                        }
                    }
                    ((_, left), (_, right)) => {
                        let (left, right) = join!(left.simplify(cx), right.simplify(cx));

                        Mul(Box::new(left), Box::new(right))
                    }
                }
            }
            Pow(base, pow) => {
                let base_const = base.clone().eval_const(cx).await;

                match base_const {
                    Some(base) => Const(base.pow(pow)),
                    None => {
                        let base = base.simplify(cx).await;

                        match pow {
                            0 => Const(1),
                            1 => base,
                            pow => Pow(Box::new(base), pow),
                        }
                    }
                }
            }
        }
    }
}

impl FromStr for Symbol {
    type Err = ParseSymbolError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use nom::IResult;

        fn constant(s: &str) -> IResult<&str, Symbol> {
            use nom::character::complete::digit1;

            let (s, digits) = digit1(s)?;
            let n = digits.parse().map_err(|_| {
                use nom::error::{Error, ErrorKind};
                use nom::Err;

                Err::Error(Error {
                    input: s,
                    code: ErrorKind::Digit,
                })
            })?;
            let r = Symbol::Const(n);

            Ok((s, r))
        }

        fn variable(s: &str) -> IResult<&str, Symbol> {
            use nom::character::complete::char;

            let (s, _) = char('x')(s)?;

            Ok((s, Symbol::Var))
        }

        fn add(s: &str) -> IResult<&str, Symbol> {
            use nom::branch::alt;
            use nom::character::complete::{char, space0};
            use nom::sequence::{delimited, separated_pair};

            let (s, (left, right)) = separated_pair(
                alt((mul, pow, par, variable, constant)),
                delimited(space0, char('+'), space0),
                alt((symbol, par)),
            )(s)?;
            let r = Symbol::Add(Box::new(left), Box::new(right));

            Ok((s, r))
        }

        fn mul(s: &str) -> IResult<&str, Symbol> {
            use nom::branch::alt;
            use nom::character::complete::{char, space0};
            use nom::sequence::{delimited, separated_pair};

            let (s, (left, right)) = separated_pair(
                alt((pow, par, variable, constant)),
                delimited(space0, char('*'), space0),
                alt((mul, pow, par, variable, constant)),
            )(s)?;
            let r = Symbol::Mul(Box::new(left), Box::new(right));

            Ok((s, r))
        }

        fn pow(s: &str) -> IResult<&str, Symbol> {
            use nom::branch::alt;
            use nom::character::complete::{char, digit1, space0};
            use nom::sequence::{delimited, separated_pair};

            let (s, (base, pow)) = separated_pair(
                alt((par, variable, constant)),
                delimited(space0, char('^'), space0),
                digit1,
            )(s)?;
            let pow = pow.parse().map_err(|_| {
                use nom::error::{Error, ErrorKind};
                use nom::Err;

                Err::Error(Error {
                    input: s,
                    code: ErrorKind::Digit,
                })
            })?;
            let r = Symbol::Pow(Box::new(base), pow);

            Ok((s, r))
        }

        fn par(s: &str) -> IResult<&str, Symbol> {
            use nom::character::complete::char;
            use nom::sequence::delimited;

            delimited(char('('), symbol, char(')'))(s)
        }

        fn symbol(s: &str) -> IResult<&str, Symbol> {
            use nom::branch::alt;

            alt((add, mul, pow, variable, constant))(s)
        }

        let (s, r) = symbol(s).map_err(|_| ParseSymbolError)?;

        if s.is_empty() {
            Ok(r)
        } else {
            Err(ParseSymbolError)
        }
    }
}

impl Display for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Symbol::Const(n) => {
                if *n < 0 {
                    write!(f, "({})", n)
                } else {
                    write!(f, "{}", n)
                }
            }
            Symbol::Var => write!(f, "x"),
            Symbol::Add(left, right) => write!(f, "({} + {})", left, right),
            Symbol::Mul(left, right) => write!(f, "({} * {})", left, right),
            Symbol::Pow(base, pow) => write!(f, "({})^{}", base, pow),
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash, InputAccess)]
struct Formula;

impl Input<String> for Formula {
    fn initial(&self) -> String {
        "0".to_string()
    }
}

#[query]
async fn function(cx: &QueryContext) -> Option<Symbol> {
    let s: String = Formula.get(cx).await;
    s.parse().ok()
}

#[query]
async fn derivative(cx: &QueryContext) -> Option<Symbol> {
    let f = function(cx).await?;

    Some(f.derivative(cx).await)
}

#[query]
async fn derivative_at(x: i64, cx: &QueryContext) -> Option<i64> {
    let df = derivative(cx).await?;

    Some(df.eval(x, cx).await)
}

#[tokio::main]
async fn main() {
    let rt = Runtime::new().await;

    rt.mutate(|cx| async move {
        Formula.set("x^2".to_string(), &cx).await;
    })
    .await;

    rt.query(|cx| async move {
        let f: String = Formula.get(&cx).await;
        println!(" f(x) = {}", f);

        match derivative(&cx).await {
            Some(df) => println!("df(x) = {}", df),
            None => println!("Failed to parse the function!"),
        }

        assert_eq!(derivative_at(1, &cx).await?, 2);
        assert_eq!(derivative_at(10, &cx).await?, 20);

        println!("Derivative seems to be correct.");

        Some(())
    })
    .await;

    rt.mutate(|cx| async move {
        Formula.set("3*x^3 + 3^3 + x".to_string(), &cx).await;
    })
    .await;

    rt.query(|cx| async move {
        let f: String = Formula.get(&cx).await;
        println!(" f(x) = {}", f);

        match derivative(&cx).await {
            Some(df) => println!("df(x) = {}", df),
            None => println!("Failed to parse the function!"),
        }

        assert_eq!(derivative_at(1, &cx).await?, 10);
        assert_eq!(derivative_at(10, &cx).await?, 901);

        println!("Derivative seems to be correct.");

        Some(())
    })
    .await;

    rt.mutate(|cx| async move {
        Formula.set("5*(x^3 + 3*x^2 + 1)^2".to_string(), &cx).await;
    })
    .await;

    rt.query(|cx| async move {
        let f: String = Formula.get(&cx).await;
        println!(" f(x) = {}", f);

        match derivative(&cx).await {
            Some(df) => println!("df(x) = {}", df),
            None => println!("Failed to parse the function!"),
        }

        assert_eq!(derivative_at(1, &cx).await?, 450);
        assert_eq!(derivative_at(10, &cx).await?, 4683600);

        println!("Derivative seems to be correct.");

        Some(())
    })
    .await;
}
