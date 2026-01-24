//! # JSON 工具模块
//!
//! 提供 JSON 序列化和反序列化的辅助工具
//!
//! # 主要组件
//! - `StupidValue<T>`: 宽松类型转换的 JSON 值包装器
//! - `my_date_format`: 日期时间格式化模块 (`%Y-%m-%d %H:%M:%S`)
//! - `my_date_format_option`: 可选日期时间格式化模块
//! - `empty_string_as_none`: 将空字符串转换为 None 的反序列化装饰器

use std::fmt::Display;
use std::str::FromStr;
use std::{borrow::Cow, fmt};

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

/// Serde 反序列化装饰器，将空字符串映射为 None
///
/// # 类型参数
/// - `D`: 反序列化器类型
/// - `T`: 目标类型，必须实现 `FromStr`
///
/// # 返回
/// - `Ok(Some(T))`: 非空字符串，成功解析为 T
/// - `Ok(None)`: 空字符串或 null
/// - `Err`: 解析失败
///
/// # 示例
///
/// ```
/// use serde::{Deserialize};
///
/// #[derive(Deserialize)]
/// struct MyStruct {
///     #[serde(deserialize_with = "axum_bootstrap::util::json::empty_string_as_none")]
///     value: Option<i32>,
/// }
/// ```
#[allow(dead_code)]
pub fn empty_string_as_none<'de, D, T>(de: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: fmt::Display,
{
    let opt = Option::<String>::deserialize(de)?;
    match opt.as_deref() {
        None | Some("") => Ok(None),
        Some(s) => FromStr::from_str(s).map_err(de::Error::custom).map(Some),
    }
}

/// 宽松类型转换的 JSON 值包装器
///
/// 可以同时接受字符串和原始类型的 JSON 值，并自动进行类型转换
///
/// # 类型参数
/// - `T`: 目标类型，必须实现 `FromStr` 和 `Deserialize`
///
/// # 使用场景
/// 当 API 返回的数据类型不一致时（有时是字符串，有时是数字），
/// 可以使用 StupidValue 来统一处理
///
/// # 示例
///
/// ```
/// use serde::{Deserialize};
/// use axum_bootstrap::util::json::StupidValue;
///
/// #[derive(Deserialize)]
/// struct Response {
///     // 可以接受 "123" 或 123
///     count: StupidValue<i32>,
/// }
///
/// let json1 = r#"{"count": "123"}"#;
/// let json2 = r#"{"count": 123}"#;
/// // 两者都能正确解析
/// ```
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StupidValue<T>(pub T);

impl<T> Serialize for StupidValue<T>
where
    T: ToString,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.to_string().serialize(serializer)
    }
}

impl<'de, T> Deserialize<'de> for StupidValue<T>
where
    T: FromStr + Deserialize<'de>,
    T::Err: Display,
{
    fn deserialize<D>(deserializer: D) -> Result<StupidValue<T>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StrOrValue<'a, T> {
            Str(Cow<'a, str>),
            Value(T),
        }

        let str_or_val = StrOrValue::<T>::deserialize(deserializer)?;
        Ok(StupidValue(match str_or_val {
            StrOrValue::Value(val) => val,
            StrOrValue::Str(s) => s.parse().map_err(serde::de::Error::custom)?,
        }))
    }
}

impl<T> From<T> for StupidValue<T> {
    fn from(val: T) -> Self {
        StupidValue(val)
    }
}

/// 日期时间格式化模块
///
/// 提供 `%Y-%m-%d %H:%M:%S` 格式的日期时间序列化和反序列化
///
/// # 使用示例
///
/// ```
/// use serde::{Serialize, Deserialize};
/// use chrono::NaiveDateTime;
///
/// #[derive(Serialize, Deserialize)]
/// struct Event {
///     #[serde(with = "axum_bootstrap::util::json::my_date_format")]
///     created_at: NaiveDateTime,
/// }
/// ```
pub mod my_date_format {
    use chrono::NaiveDateTime;
    use serde::{self, Deserialize, Deserializer, Serializer};

    /// 日期时间格式: `YYYY-MM-DD HH:MM:SS`
    const FORMAT: &str = "%Y-%m-%d %H:%M:%S";

    /// 将 NaiveDateTime 序列化为字符串
    ///
    /// # 参数
    /// - `date`: 要序列化的日期时间
    /// - `serializer`: 序列化器
    ///
    /// # 格式
    /// `YYYY-MM-DD HH:MM:SS`
    pub fn serialize<S>(date: &NaiveDateTime, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let s = format!("{}", date.format(FORMAT));
        serializer.serialize_str(&s)
    }

    /// 从字符串反序列化为 NaiveDateTime
    ///
    /// # 参数
    /// - `deserializer`: 反序列化器
    ///
    /// # 返回
    /// - `Ok(NaiveDateTime)`: 成功解析的日期时间
    /// - `Err`: 格式错误
    pub fn deserialize<'de, D>(deserializer: D) -> Result<NaiveDateTime, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let dt = NaiveDateTime::parse_from_str(&s, FORMAT).map_err(serde::de::Error::custom)?;
        Ok(dt)
    }
}

/// 可选日期时间格式化模块
///
/// 提供 `Option<NaiveDateTime>` 的序列化和反序列化，格式为 `%Y-%m-%d %H:%M:%S`
///
/// # 使用示例
///
/// ```
/// use serde::{Serialize, Deserialize};
/// use chrono::NaiveDateTime;
///
/// #[derive(Serialize, Deserialize)]
/// struct Event {
///     #[serde(with = "axum_bootstrap::util::json::my_date_format_option")]
///     updated_at: Option<NaiveDateTime>,
/// }
/// ```
pub mod my_date_format_option {
    use super::my_date_format;
    use chrono::NaiveDateTime;
    use serde::{self, Deserialize, Deserializer, Serializer};

    /// 日期时间格式: `YYYY-MM-DD HH:MM:SS`
    const FORMAT: &str = "%Y-%m-%d %H:%M:%S";

    /// 将 `Option<NaiveDateTime>` 序列化为字符串或 null
    ///
    /// # 参数
    /// - `opt`: 可选的日期时间
    /// - `serializer`: 序列化器
    ///
    /// # 行为
    /// - `Some(date)`: 序列化为格式化字符串
    /// - `None`: 序列化为 null
    pub fn serialize<S>(opt: &Option<NaiveDateTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match *opt {
            Some(ref dt) => my_date_format::serialize(dt, serializer),
            None => serializer.serialize_none(),
        }
    }

    /// 从字符串或 null 反序列化为 `Option<NaiveDateTime>`
    ///
    /// # 参数
    /// - `deserializer`: 反序列化器
    ///
    /// # 返回
    /// - `Ok(Some(NaiveDateTime))`: 成功解析的日期时间
    /// - `Ok(None)`: 输入为 null
    /// - `Err`: 格式错误
    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<NaiveDateTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        match Option::<String>::deserialize(deserializer)? {
            Some(s) => {
                let dt = NaiveDateTime::parse_from_str(&s, FORMAT).map_err(serde::de::Error::custom)?;
                Ok(Some(dt))
            }
            None => Ok(None),
        }
    }
}
