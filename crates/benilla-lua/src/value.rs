//! Values, NaN-boxed into 8 bytes. Every collectable is a handle into the
//! [`crate::heap::Heap`]'s arenas, so a value is `Copy` and the whole state can be cloned.
//!
//! A number is its own bits. Everything else lives at or above [`TAGGED`], a range of negative
//! quiet NaNs no number here takes: the hardware's default NaN is `0xFFF8…`, arithmetic on it
//! keeps it, and a NaN from outside is canonicalised by [`Value::Num`]. Above `TAGGED`, three
//! bits name the type and 47 bits carry the payload. One value is one 8-byte load or store, so
//! a value written and read back never splits a store across a load (x86's store forwarding).

/// The first tagged pattern: `0xFFFC_0000_0000_0000`.
const TAGGED: u64 = 0xFFFC_0000_0000_0000;
const TAG_SHIFT: u32 = 47;
const PAYLOAD: u64 = (1 << TAG_SHIFT) - 1;

const T_NIL: u64 = 0;
const T_BOOL: u64 = 1;
const T_STR: u64 = 2;
const T_TABLE: u64 = 3;
const T_FUNC: u64 = 4;
const T_USER: u64 = 5;
const T_LIGHT: u64 = 6;

const fn tagged(tag: u64, payload: u64) -> u64 {
    TAGGED | (tag << TAG_SHIFT) | payload
}

/// A string's id bits in a value; the rest of the payload is its hash's low bits.
pub const STR_ID_BITS: u32 = 24;
/// The hash bits a string value carries, enough to place it in a table of `2^23` nodes.
pub const STR_HASH_MASK: u32 = (1 << (47 - STR_ID_BITS)) - 1;

/// A string handle with its 5.0 hash's low bits beside it, so a table can place a string key
/// without a trip to the heap. Strings are interned: one handle per distinct byte string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StrRef {
    pub id: u32,
    /// `luaS_hash & STR_HASH_MASK`.
    pub hash: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TableRef(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FuncRef(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UserRef(pub u32);

/// A Lua value in 8 bytes. Build one with the constructors named after the types
/// (`Value::Num(1.0)`, `Value::Nil`); read one with [`Value::unpack`] or the `as_*` accessors.
/// `==` compares bits, which is raw equality for everything but numbers (`0 == -0`, NaN):
/// use [`Value::raw_eq`] for Lua's.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Value(u64);

/// A value unpacked, for matching.
#[derive(Clone, Copy, Debug)]
pub enum V {
    Nil,
    Bool(bool),
    Num(f64),
    Str(StrRef),
    Table(TableRef),
    /// A Lua closure or a native function.
    Func(FuncRef),
    /// A full userdata: a frame or other host object, with a metatable.
    User(UserRef),
    /// 5.0's light userdata: a bare host pointer, compared by value.
    Light(usize),
}

impl Default for Value {
    fn default() -> Self {
        Value::Nil
    }
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.unpack().fmt(f)
    }
}

#[allow(non_upper_case_globals, non_snake_case)]
impl Value {
    pub const Nil: Value = Value(tagged(T_NIL, 0));
    pub const False: Value = Value(tagged(T_BOOL, 0));
    pub const True: Value = Value(tagged(T_BOOL, 1));
    /// 5.0's `LUA_TNONE` as a table key, which a weak table's clearing leaves (`removekey`,
    /// `0x6f79d0`): the canonical NaN, which no lookup matches and no write makes a key.
    pub(crate) const NONE_KEY: Value = Value(0x7FF8_0000_0000_0000);

    /// A number; a NaN becomes the canonical one of its sign.
    #[inline(always)]
    pub fn Num(n: f64) -> Value {
        if n.is_nan() {
            Value((n.to_bits() & (1 << 63)) | 0x7FF8_0000_0000_0000)
        } else {
            Value(n.to_bits())
        }
    }

    /// A number from arithmetic on numbers already canonical, which keeps them so.
    #[inline(always)]
    pub fn num_raw(n: f64) -> Value {
        debug_assert!(n.to_bits() < TAGGED);
        Value(n.to_bits())
    }

    #[inline(always)]
    pub fn Bool(b: bool) -> Value {
        Value(tagged(T_BOOL, b as u64))
    }

    #[inline(always)]
    pub fn Str(s: StrRef) -> Value {
        debug_assert!(s.id < 1 << STR_ID_BITS && s.hash <= STR_HASH_MASK);
        Value(tagged(
            T_STR,
            (s.id as u64) | ((s.hash as u64) << STR_ID_BITS),
        ))
    }

    #[inline(always)]
    pub fn Table(t: TableRef) -> Value {
        Value(tagged(T_TABLE, t.0 as u64))
    }

    #[inline(always)]
    pub fn Func(f: FuncRef) -> Value {
        Value(tagged(T_FUNC, f.0 as u64))
    }

    #[inline(always)]
    pub fn User(u: UserRef) -> Value {
        Value(tagged(T_USER, u.0 as u64))
    }

    #[inline(always)]
    pub fn Light(p: usize) -> Value {
        Value(tagged(T_LIGHT, p as u64 & PAYLOAD))
    }

    #[inline(always)]
    fn tag(self) -> u64 {
        (self.0 >> TAG_SHIFT) & 7
    }

    #[inline(always)]
    fn payload(self) -> u64 {
        self.0 & PAYLOAD
    }

    #[inline(always)]
    pub fn unpack(self) -> V {
        if self.0 < TAGGED {
            return V::Num(f64::from_bits(self.0));
        }
        let p = self.payload();
        match self.tag() {
            T_NIL => V::Nil,
            T_BOOL => V::Bool(p != 0),
            T_STR => V::Str(StrRef {
                id: (p & ((1 << STR_ID_BITS) - 1)) as u32,
                hash: (p >> STR_ID_BITS) as u32,
            }),
            T_TABLE => V::Table(TableRef(p as u32)),
            T_FUNC => V::Func(FuncRef(p as u32)),
            T_USER => V::User(UserRef(p as u32)),
            _ => V::Light(p as usize),
        }
    }

    /// A string's hash bits, for a value known to be a string.
    #[inline(always)]
    pub fn str_hash_bits(self) -> u32 {
        debug_assert!(matches!(self.unpack(), V::Str(_)));
        (self.payload() >> STR_ID_BITS) as u32
    }

    #[inline(always)]
    pub fn is_num(self) -> bool {
        self.0 < TAGGED
    }

    #[inline(always)]
    pub fn as_num(self) -> Option<f64> {
        if self.0 < TAGGED {
            Some(f64::from_bits(self.0))
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn as_str(self) -> Option<StrRef> {
        match self.unpack() {
            V::Str(s) => Some(s),
            _ => None,
        }
    }

    #[inline(always)]
    pub fn as_table(self) -> Option<TableRef> {
        if self.0 >> TAG_SHIFT == TAGGED >> TAG_SHIFT | T_TABLE {
            Some(TableRef(self.payload() as u32))
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn as_func(self) -> Option<FuncRef> {
        if self.0 >> TAG_SHIFT == TAGGED >> TAG_SHIFT | T_FUNC {
            Some(FuncRef(self.payload() as u32))
        } else {
            None
        }
    }

    #[inline(always)]
    pub fn is_nil(self) -> bool {
        self.0 == Value::Nil.0
    }

    /// Lua truth: only `nil` and `false` are false.
    #[inline(always)]
    pub fn truthy(self) -> bool {
        self.0 != Value::Nil.0 && self.0 != Value::False.0
    }

    /// `luaO_rawequalObj`: bits, but numbers by value.
    #[inline(always)]
    pub fn raw_eq(self, other: Value) -> bool {
        if self.0 < TAGGED && other.0 < TAGGED {
            f64::from_bits(self.0) == f64::from_bits(other.0)
        } else {
            self.0 == other.0
        }
    }

    /// `luaT_typenames`, 5.0's spelling.
    pub fn type_name(self) -> &'static str {
        if self.0 < TAGGED {
            return "number";
        }
        match self.tag() {
            T_NIL => "nil",
            T_BOOL => "boolean",
            T_STR => "string",
            T_TABLE => "table",
            T_FUNC => "function",
            _ => "userdata",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip() {
        for n in [0.0, -0.0, 1.5, -1e300, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(Value::Num(n).as_num().unwrap().to_bits(), n.to_bits());
        }
        let nan = Value::Num(f64::NAN);
        assert!(nan.is_num() && nan.as_num().unwrap().is_nan());
        assert!(Value::Num(-f64::NAN).as_num().unwrap().is_sign_negative());
        assert!(!nan.raw_eq(nan));
        assert!(Value::Num(0.0).raw_eq(Value::Num(-0.0)));
        let s = StrRef {
            id: (1 << STR_ID_BITS) - 1,
            hash: STR_HASH_MASK,
        };
        assert!(matches!(Value::Str(s).unpack(), V::Str(x) if x == s));
        assert_eq!(Value::Table(TableRef(7)).as_table(), Some(TableRef(7)));
        assert_eq!(Value::Func(FuncRef(7)).as_table(), None);
        assert!(Value::Nil.is_nil() && !Value::Nil.truthy() && !Value::False.truthy());
        assert!(Value::True.truthy() && Value::Num(0.0).truthy());
        assert_eq!(std::mem::size_of::<Value>(), 8);
    }
}
