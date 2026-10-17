//! mlua's conversion traits and the impls benilla relies on, with mlua's rules: an integer target
//! truncates a number and reads a numeric string; a float target coerces; a Rust string coerces a
//! number and needs UTF-8; `bool` is Lua truth; `Option` is nil or the value.

use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasher, Hash};
use std::sync::Arc;

use crate::{
    BorrowedBytes, BorrowedStr, Error, Function, LightUserData, Lua, MultiValue, Result, String,
    Table, Value, Variadic,
};

pub trait IntoLua: Sized {
    fn into_lua(self, lua: &Lua) -> Result<Value>;

    /// This value as the VM's, with no handle made, for a type whose conversion runs no Lua: a
    /// scalar, a string, a handle. The caller uses the value before any Lua runs, which is when
    /// the collector could free what no handle holds. Any other type hands itself back for
    /// `into_lua`.
    #[doc(hidden)]
    fn into_raw(self, _: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
        Err(self)
    }
}

pub trait FromLua: Sized {
    fn from_lua(value: Value, lua: &Lua) -> Result<Self>;

    /// `from_lua` of the VM value `v`, which the caller keeps alive: a type that reads it in
    /// place answers without making a handle, and anything else goes through `from_lua`.
    #[doc(hidden)]
    fn from_raw(v: benilla_lua::Value, lua: &Lua) -> Result<Self> {
        Self::from_lua(lua.wrap(v), lua)
    }
}

pub trait IntoLuaMulti: Sized {
    fn into_lua_multi(self, lua: &Lua) -> Result<MultiValue>;

    /// A callback's results, pushed onto the VM's stack as they convert; answers how many. A
    /// single value or a tuple goes without the `MultiValue` in between.
    #[doc(hidden)]
    fn push_results(self, lua: &Lua) -> Result<usize> {
        let m = self.into_lua_multi(lua)?;
        for v in m.iter() {
            let x = lua.unwrap(v);
            lua.vm().push(x);
        }
        Ok(m.len())
    }
}

pub trait FromLuaMulti: Sized {
    fn from_lua_multi(values: MultiValue, lua: &Lua) -> Result<Self>;

    /// A callback's arguments: as `from_lua_multi`, with a failure named by its position.
    fn from_lua_args(values: MultiValue, lua: &Lua) -> Result<Self> {
        Self::from_lua_multi(values, lua)
    }

    /// `from_lua_args` of a callback's arguments as the VM holds them, rooted on its stack for
    /// the whole call: an element read in place (`FromLua::from_raw`) makes no handle.
    #[doc(hidden)]
    fn from_raw_args(args: &[benilla_lua::Value], lua: &Lua) -> Result<Self> {
        Self::from_lua_args(args.iter().map(|&v| lua.wrap(v)).collect(), lua)
    }

    /// `from_lua_multi` of values rooted for the whole conversion, as `from_raw_args`.
    #[doc(hidden)]
    fn from_raw_multi(values: &[benilla_lua::Value], lua: &Lua) -> Result<Self> {
        Self::from_lua_multi(values.iter().map(|&v| lua.wrap(v)).collect(), lua)
    }
}

fn conversion(from: &'static str, to: &str, message: Option<&str>) -> Error {
    Error::FromLuaConversionError {
        from,
        to: to.to_string(),
        message: message.map(str::to_string),
    }
}

// ── values and handles ───────────────────────────────────────────────────────────────────────

impl IntoLua for Value {
    fn into_lua(self, _: &Lua) -> Result<Value> {
        Ok(self)
    }

    fn into_raw(self, lua: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
        Ok(Ok(lua.unwrap(&self)))
    }
}

impl IntoLua for &Value {
    fn into_lua(self, _: &Lua) -> Result<Value> {
        Ok(self.clone())
    }

    fn into_raw(self, lua: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
        Ok(Ok(lua.unwrap(self)))
    }
}

impl FromLua for Value {
    fn from_lua(v: Value, _: &Lua) -> Result<Self> {
        Ok(v)
    }
}

macro_rules! handle {
    ($t:ident) => {
        impl IntoLua for $t {
            fn into_lua(self, _: &Lua) -> Result<Value> {
                Ok(Value::$t(self))
            }

            fn into_raw(self, lua: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
                Ok(Ok(lua.unref(&self.0)))
            }
        }

        impl IntoLua for &$t {
            fn into_lua(self, _: &Lua) -> Result<Value> {
                Ok(Value::$t(self.clone()))
            }

            fn into_raw(self, lua: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
                Ok(Ok(lua.unref(&self.0)))
            }
        }
    };
}

handle!(Table);
handle!(Function);

impl IntoLua for String {
    fn into_lua(self, _: &Lua) -> Result<Value> {
        Ok(Value::String(self))
    }

    fn into_raw(self, lua: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
        Ok(Ok(lua.unref(&self.0)))
    }
}

impl IntoLua for &String {
    fn into_lua(self, _: &Lua) -> Result<Value> {
        Ok(Value::String(self.clone()))
    }

    fn into_raw(self, lua: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
        Ok(Ok(lua.unref(&self.0)))
    }
}

impl FromLua for Table {
    fn from_lua(v: Value, _: &Lua) -> Result<Self> {
        match v {
            Value::Table(t) => Ok(t),
            other => Err(conversion(other.type_name(), "table", None)),
        }
    }
}

impl FromLua for Function {
    fn from_lua(v: Value, _: &Lua) -> Result<Self> {
        match v {
            Value::Function(f) => Ok(f),
            other => Err(conversion(other.type_name(), "function", None)),
        }
    }
}

impl FromLua for String {
    fn from_lua(v: Value, lua: &Lua) -> Result<Self> {
        let ty = v.type_name();
        lua.coerce_string(v)?
            .ok_or_else(|| conversion(ty, "string", Some("expected string or number")))
    }
}

impl FromLua for LightUserData {
    fn from_lua(v: Value, _: &Lua) -> Result<Self> {
        match v {
            Value::LightUserData(l) => Ok(l),
            other => Err(conversion(other.type_name(), "lightuserdata", None)),
        }
    }
}

impl IntoLua for LightUserData {
    fn into_lua(self, _: &Lua) -> Result<Value> {
        Ok(Value::LightUserData(self))
    }
}

impl IntoLua for Error {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::String(lua.create_string(self.lua_message())?))
    }
}

// ── scalars ──────────────────────────────────────────────────────────────────────────────────

impl IntoLua for bool {
    fn into_lua(self, _: &Lua) -> Result<Value> {
        Ok(Value::Boolean(self))
    }

    fn into_raw(self, _: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
        Ok(Ok(benilla_lua::Value::Bool(self)))
    }
}

impl FromLua for bool {
    fn from_lua(v: Value, _: &Lua) -> Result<Self> {
        Ok(match v {
            Value::Nil => false,
            Value::Boolean(b) => b,
            _ => true,
        })
    }

    fn from_raw(v: benilla_lua::Value, _: &Lua) -> Result<Self> {
        Ok(v.truthy())
    }
}

/// mlua's `num_traits::cast` from a float: truncation, `None` out of range.
fn float_to_int<T: TryFrom<i128>>(n: f64) -> Option<T> {
    if !n.is_finite() {
        return None;
    }
    let t = n.trunc();
    if t < i128::MIN as f64 || t > i128::MAX as f64 {
        return None;
    }
    T::try_from(t as i128).ok()
}

macro_rules! int {
    ($($t:ty),*) => {$(
        impl IntoLua for $t {
            fn into_lua(self, _: &Lua) -> Result<Value> {
                Ok(match i64::try_from(self) {
                    Ok(i) => Value::Integer(i),
                    Err(_) => Value::Number(self as f64),
                })
            }

            // `Integer(i)` is the VM's `i as f64`, which is `self as f64`.
            fn into_raw(self, _: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
                Ok(Ok(benilla_lua::Value::Num(self as f64)))
            }
        }

        impl FromLua for $t {
            fn from_lua(v: Value, lua: &Lua) -> Result<Self> {
                let ty = v.type_name();
                let out = match v {
                    Value::Integer(i) => <$t>::try_from(i).ok(),
                    Value::Number(n) => float_to_int::<$t>(n),
                    other => match lua.coerce_number(other)? {
                        Some(n) => float_to_int::<$t>(n),
                        None => {
                            return Err(conversion(
                                ty,
                                stringify!($t),
                                Some("expected number or string coercible to number"),
                            ))
                        }
                    },
                };
                out.ok_or_else(|| conversion(ty, stringify!($t), Some("out of range")))
            }

            // A number in range converts as `from_lua` converts it, `Integer` or `Number`;
            // anything else, an error included, goes through `from_lua`.
            fn from_raw(v: benilla_lua::Value, lua: &Lua) -> Result<Self> {
                if let Some(x) = v.as_num().and_then(float_to_int::<$t>) {
                    return Ok(x);
                }
                Self::from_lua(lua.wrap(v), lua)
            }
        }
    )*};
}

int!(i8, u8, i16, u16, i32, u32, i64, u64, i128, u128, isize, usize);

macro_rules! float {
    ($($t:ty),*) => {$(
        impl IntoLua for $t {
            fn into_lua(self, _: &Lua) -> Result<Value> {
                Ok(Value::Number(self as f64))
            }

            fn into_raw(self, _: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
                Ok(Ok(benilla_lua::Value::Num(self as f64)))
            }
        }

        impl FromLua for $t {
            fn from_lua(v: Value, lua: &Lua) -> Result<Self> {
                let ty = v.type_name();
                lua.coerce_number(v)?.map(|n| n as $t).ok_or_else(|| {
                    conversion(ty, stringify!($t), Some("expected number or string coercible to number"))
                })
            }

            fn from_raw(v: benilla_lua::Value, lua: &Lua) -> Result<Self> {
                match v.as_num() {
                    Some(n) => Ok(n as $t),
                    None => Self::from_lua(lua.wrap(v), lua),
                }
            }
        }
    )*};
}

float!(f32, f64);

// ── text ─────────────────────────────────────────────────────────────────────────────────────

impl IntoLua for std::string::String {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::String(lua.create_string(self)?))
    }

    fn into_raw(self, lua: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
        Ok(lua.raw_string(self.as_bytes()))
    }
}

impl IntoLua for &std::string::String {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::String(lua.create_string(self)?))
    }

    fn into_raw(self, lua: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
        Ok(lua.raw_string(self.as_bytes()))
    }
}

impl IntoLua for &str {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::String(lua.create_string(self)?))
    }

    fn into_raw(self, lua: &Lua) -> std::result::Result<Result<benilla_lua::Value>, Self> {
        Ok(lua.raw_string(self.as_bytes()))
    }
}

impl IntoLua for std::borrow::Cow<'_, str> {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::String(lua.create_string(self.as_bytes())?))
    }
}

impl IntoLua for Box<str> {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::String(lua.create_string(self.as_bytes())?))
    }
}

impl IntoLua for &[u8] {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::String(lua.create_string(self)?))
    }
}

impl IntoLua for BorrowedStr<'_> {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::String(lua.create_string(self.as_bytes())?))
    }
}

impl IntoLua for BorrowedBytes<'_> {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::String(lua.create_string(&*self)?))
    }
}

impl IntoLua for char {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        let mut b = [0u8; 4];
        Ok(Value::String(lua.create_string(self.encode_utf8(&mut b))?))
    }
}

impl FromLua for std::string::String {
    fn from_lua(v: Value, lua: &Lua) -> Result<Self> {
        let ty = v.type_name();
        let s = lua
            .coerce_string(v)?
            .ok_or_else(|| conversion(ty, "String", Some("expected string or number")))?;
        Ok((*s.to_str()?).to_owned())
    }

    // A UTF-8 string read in place; a number to coerce, or an error, through `from_lua`.
    fn from_raw(v: benilla_lua::Value, lua: &Lua) -> Result<Self> {
        if let Some(s) = v.as_str() {
            if let Ok(s) = std::str::from_utf8(lua.vm().heap.str(s)) {
                return Ok(s.to_owned());
            }
        }
        Self::from_lua(lua.wrap(v), lua)
    }
}

impl FromLua for Box<str> {
    fn from_lua(v: Value, lua: &Lua) -> Result<Self> {
        Ok(std::string::String::from_lua(v, lua)?.into_boxed_str())
    }
}

impl FromLua for Arc<str> {
    fn from_lua(v: Value, lua: &Lua) -> Result<Self> {
        Ok(std::string::String::from_lua(v, lua)?.into())
    }
}

// ── containers ───────────────────────────────────────────────────────────────────────────────

impl<T: IntoLua> IntoLua for Option<T> {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        match self {
            Some(v) => v.into_lua(lua),
            None => Ok(Value::Nil),
        }
    }
}

impl<T: FromLua> FromLua for Option<T> {
    fn from_lua(v: Value, lua: &Lua) -> Result<Self> {
        match v {
            Value::Nil => Ok(None),
            v => T::from_lua(v, lua).map(Some),
        }
    }

    fn from_raw(v: benilla_lua::Value, lua: &Lua) -> Result<Self> {
        if v.is_nil() {
            return Ok(None);
        }
        T::from_raw(v, lua).map(Some)
    }
}

impl<T: IntoLua> IntoLua for Vec<T> {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::Table(lua.create_sequence_from(self)?))
    }
}

impl<T: FromLua> FromLua for Vec<T> {
    fn from_lua(v: Value, _: &Lua) -> Result<Self> {
        match v {
            Value::Table(t) => t.sequence_values::<T>().collect(),
            other => Err(conversion(other.type_name(), "Vec", Some("expected table"))),
        }
    }
}

impl<T: IntoLua, const N: usize> IntoLua for [T; N] {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::Table(lua.create_sequence_from(self)?))
    }
}

impl<K: Eq + Hash + IntoLua, V2: IntoLua, S: BuildHasher> IntoLua for HashMap<K, V2, S> {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::Table(lua.create_table_from(self)?))
    }
}

impl<K: Eq + Hash + FromLua, V2: FromLua, S: BuildHasher + Default> FromLua for HashMap<K, V2, S> {
    fn from_lua(v: Value, _: &Lua) -> Result<Self> {
        match v {
            Value::Table(t) => t.pairs::<K, V2>().collect(),
            other => Err(conversion(
                other.type_name(),
                "HashMap",
                Some("expected table"),
            )),
        }
    }
}

impl<K: Ord + IntoLua, V2: IntoLua> IntoLua for BTreeMap<K, V2> {
    fn into_lua(self, lua: &Lua) -> Result<Value> {
        Ok(Value::Table(lua.create_table_from(self)?))
    }
}

impl<K: Ord + FromLua, V2: FromLua> FromLua for BTreeMap<K, V2> {
    fn from_lua(v: Value, _: &Lua) -> Result<Self> {
        match v {
            Value::Table(t) => t.pairs::<K, V2>().collect(),
            other => Err(conversion(
                other.type_name(),
                "BTreeMap",
                Some("expected table"),
            )),
        }
    }
}

// ── multiple values ──────────────────────────────────────────────────────────────────────────

impl<T: IntoLua> IntoLuaMulti for T {
    fn into_lua_multi(self, lua: &Lua) -> Result<MultiValue> {
        let mut m = MultiValue::with_capacity(1);
        m.push_back(self.into_lua(lua)?);
        Ok(m)
    }

    fn push_results(self, lua: &Lua) -> Result<usize> {
        let v = self.into_lua(lua)?;
        let x = lua.unwrap(&v);
        lua.vm().push(x);
        Ok(1)
    }
}

impl<T: FromLua> FromLuaMulti for T {
    fn from_lua_multi(mut values: MultiValue, lua: &Lua) -> Result<Self> {
        T::from_lua(values.pop_front().unwrap_or(Value::Nil), lua)
    }

    fn from_lua_args(mut values: MultiValue, lua: &Lua) -> Result<Self> {
        T::from_lua(values.pop_front().unwrap_or(Value::Nil), lua).map_err(|e| bad_arg(1, e))
    }

    fn from_raw_args(args: &[benilla_lua::Value], lua: &Lua) -> Result<Self> {
        T::from_raw(first_raw(args), lua).map_err(|e| bad_arg(1, e))
    }

    fn from_raw_multi(values: &[benilla_lua::Value], lua: &Lua) -> Result<Self> {
        T::from_raw(first_raw(values), lua)
    }
}

/// The first of `values`, or nil: `pop_front().unwrap_or(Value::Nil)` over VM values.
fn first_raw(values: &[benilla_lua::Value]) -> benilla_lua::Value {
    values.first().copied().unwrap_or(benilla_lua::Value::Nil)
}

impl IntoLuaMulti for MultiValue {
    fn into_lua_multi(self, _: &Lua) -> Result<MultiValue> {
        Ok(self)
    }
}

impl FromLuaMulti for MultiValue {
    fn from_lua_multi(values: MultiValue, _: &Lua) -> Result<Self> {
        Ok(values)
    }
}

impl<T: IntoLua> IntoLuaMulti for Variadic<T> {
    fn into_lua_multi(self, lua: &Lua) -> Result<MultiValue> {
        self.into_iter().map(|v| v.into_lua(lua)).collect()
    }
}

impl<T: FromLua> FromLuaMulti for Variadic<T> {
    fn from_lua_multi(values: MultiValue, lua: &Lua) -> Result<Self> {
        values.into_iter().map(|v| T::from_lua(v, lua)).collect()
    }
}

impl IntoLuaMulti for () {
    fn into_lua_multi(self, _: &Lua) -> Result<MultiValue> {
        Ok(MultiValue::new())
    }

    fn push_results(self, _: &Lua) -> Result<usize> {
        Ok(0)
    }
}

impl FromLuaMulti for () {
    fn from_lua_multi(_: MultiValue, _: &Lua) -> Result<Self> {
        Ok(())
    }

    fn from_raw_args(_: &[benilla_lua::Value], _: &Lua) -> Result<Self> {
        Ok(())
    }

    fn from_raw_multi(_: &[benilla_lua::Value], _: &Lua) -> Result<Self> {
        Ok(())
    }
}

impl<T: IntoLuaMulti, E: Into<Error>> IntoLuaMulti for std::result::Result<T, E> {
    fn into_lua_multi(self, lua: &Lua) -> Result<MultiValue> {
        match self {
            Ok(v) => v.into_lua_multi(lua),
            Err(e) => {
                let mut m = MultiValue::new();
                m.push_back(Value::Nil);
                m.push_back(e.into().into_lua(lua)?);
                Ok(m)
            }
        }
    }
}

fn bad_arg(pos: usize, cause: Error) -> Error {
    Error::BadArgument {
        to: None,
        pos,
        name: None,
        cause: Arc::new(cause),
    }
}

/// A tuple's last element takes every value left: `(Table, MultiValue)`, `(i64, Variadic<T>)`.
macro_rules! tuple {
    ($($name:ident $idx:tt,)* ; $last:ident $lidx:tt) => {
        impl<$($name: IntoLua,)* $last: IntoLuaMulti> IntoLuaMulti for ($($name,)* $last,) {
            fn into_lua_multi(self, lua: &Lua) -> Result<MultiValue> {
                let mut m = MultiValue::new();
                $( m.push_back(self.$idx.into_lua(lua)?); )*
                m.extend(self.$lidx.into_lua_multi(lua)?);
                Ok(m)
            }

            fn push_results(self, lua: &Lua) -> Result<usize> {
                $(
                    let v = self.$idx.into_lua(lua)?;
                    let x = lua.unwrap(&v);
                    lua.vm().push(x);
                )*
                Ok($lidx + self.$lidx.push_results(lua)?)
            }
        }

        #[allow(non_snake_case, unused_mut)]
        impl<$($name: FromLua,)* $last: FromLuaMulti> FromLuaMulti for ($($name,)* $last,) {
            fn from_lua_multi(mut values: MultiValue, lua: &Lua) -> Result<Self> {
                $( let $name = $name::from_lua(values.pop_front().unwrap_or(Value::Nil), lua)?; )*
                let last = $last::from_lua_multi(values, lua)?;
                Ok(($($name,)* last,))
            }

            fn from_lua_args(mut values: MultiValue, lua: &Lua) -> Result<Self> {
                $( let $name = $name::from_lua(values.pop_front().unwrap_or(Value::Nil), lua)
                    .map_err(|e| bad_arg($idx + 1, e))?; )*
                let last = $last::from_lua_multi(values, lua).map_err(|e| bad_arg($lidx + 1, e))?;
                Ok(($($name,)* last,))
            }

            fn from_raw_args(args: &[benilla_lua::Value], lua: &Lua) -> Result<Self> {
                $( let $name = $name::from_raw(first_raw(args.get($idx..).unwrap_or(&[])), lua)
                    .map_err(|e| bad_arg($idx + 1, e))?; )*
                let rest = args.get($lidx..).unwrap_or(&[]);
                let last = $last::from_raw_multi(rest, lua).map_err(|e| bad_arg($lidx + 1, e))?;
                Ok(($($name,)* last,))
            }
        }
    };
}

tuple!(; A 0);
tuple!(A 0, ; B 1);
tuple!(A 0, B 1, ; C 2);
tuple!(A 0, B 1, C 2, ; D 3);
tuple!(A 0, B 1, C 2, D 3, ; E 4);
tuple!(A 0, B 1, C 2, D 3, E 4, ; F 5);
tuple!(A 0, B 1, C 2, D 3, E 4, F 5, ; G 6);
tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, ; H 7);
tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, ; I 8);
tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, ; J 9);
tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, ; K 10);
tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, ; L 11);
tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11, ; M 12);
tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11, M 12, ; N 13);
tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11, M 12, N 13, ; O 14);
tuple!(A 0, B 1, C 2, D 3, E 4, F 5, G 6, H 7, I 8, J 9, K 10, L 11, M 12, N 13, O 14, ; P 15);
