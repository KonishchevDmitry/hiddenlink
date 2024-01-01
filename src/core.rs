pub type EmptyResult = GenericResult<()>;
pub type GenericResult<T> = Result<T, GenericError>;
pub type GenericError = Box<dyn ::std::error::Error + Send + Sync>;

#[macro_export]
macro_rules! Err {
    ($($arg:tt)*) => (::std::result::Result::Err(format!($($arg)*).into()))
}

pub trait ResultTools {
    type Value;
    type Error;

    fn ok_or_handle_error<F: FnOnce(Self::Error)>(self, op: F) -> Option<Self::Value>;
}

impl<T, E> ResultTools for Result<T, E> {
    type Value = T;
    type Error = E;

    fn ok_or_handle_error<F: FnOnce(Self::Error)>(self, op: F) -> Option<Self::Value> {
        match self {
            Ok(value) => Some(value),
            Err(err) => {
                op(err);
                None
            }
        }
    }
}