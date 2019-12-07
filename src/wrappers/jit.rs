//! JIT interface to run model trained/saved using PyTorch Python API.
use super::utils::{path_to_cstring, ptr_to_string};
use crate::Tensor;
use failure::Fallible;
use libc::c_int;
use std::borrow::Borrow;
use torch_sys::*;

/// Argument and output values for JIT models.
#[derive(Debug, PartialEq)]
pub enum IValue {
    None,
    Tensor(crate::Tensor),
    Double(f64),
    Int(i64),
    Bool(bool),
    Tuple(Vec<IValue>),
    String(String),
}

impl IValue {
    pub(super) fn to_c(&self) -> Fallible<*mut CIValue> {
        let c = unsafe_torch_err!({
            match self {
                IValue::Tensor(tensor) => ati_tensor(tensor.c_tensor),
                IValue::Int(i) => ati_int(*i),
                IValue::None => ati_none(),
                IValue::Double(f) => ati_double(*f),
                IValue::Bool(b) => ati_bool(if *b { 1 } else { 0 }),
                IValue::Tuple(v) => {
                    let v = v.iter().map(Self::to_c).collect::<Fallible<Vec<_>>>()?;
                    let tuple = ati_tuple(v.as_ptr(), v.len() as c_int);
                    for x in v {
                        ati_free(x);
                    }

                    tuple
                }
                IValue::String(string) => {
                    let c_str = std::ffi::CString::new(string.as_str())?;
                    ati_string(c_str.as_ptr())
                }
            }
        });
        Ok(c)
    }

    // This consumes the pointer and frees the associated memory.
    pub(super) fn of_c(c_ivalue: *mut CIValue) -> Fallible<Self> {
        let tag = unsafe_torch_err!({ ati_tag(c_ivalue) });
        let v = match tag {
            0 => IValue::None,
            1 => {
                let c_tensor = unsafe_torch_err!({ ati_to_tensor(c_ivalue) });
                IValue::Tensor(crate::Tensor { c_tensor })
            }
            2 => IValue::Double(unsafe_torch_err!({ ati_to_double(c_ivalue) })),
            3 => IValue::Int(unsafe_torch_err!({ ati_to_int(c_ivalue) })),
            4 => {
                let b = unsafe_torch_err!({ ati_to_bool(c_ivalue) });
                ensure!(b >= 0, "unexpected bool value {}", b);
                IValue::Bool(b != 0)
            }
            5 => {
                let len = unsafe_torch_err!({ ati_tuple_length(c_ivalue) });
                let mut c_ivalues: Vec<_> =
                    (0..len).map(|_| std::ptr::null_mut::<CIValue>()).collect();
                unsafe_torch_err!(ati_to_tuple(c_ivalue, c_ivalues.as_mut_ptr(), len));
                let vec: Result<Vec<_>, _> = c_ivalues
                    .iter()
                    .map(|&c_ivalue| (Self::of_c(c_ivalue)))
                    .collect();
                IValue::Tuple(vec?)
            }
            9 => {
                let ptr = unsafe_torch_err!({ ati_to_string(c_ivalue) });
                let string = unsafe { ptr_to_string(ptr) }.unwrap(); // TODO: better error handling
                IValue::String(string)
            }
            _ => Err(format_err!("unhandled tag {}", tag))?,
        };
        unsafe_torch_err!({ ati_free(c_ivalue) });
        Ok(v)
    }
}

/// A jit PyTorch module.
///
/// These modules can be created via the
/// [TorchScript python api](https://pytorch.org/docs/stable/jit.html).
#[derive(Debug)]
pub struct CModule {
    pub(super) c_module: *mut CModule_,
}

unsafe impl Send for CModule {}

unsafe impl Sync for CModule {}

impl Drop for CModule {
    fn drop(&mut self) {
        unsafe_torch!({ atm_free(self.c_module) })
    }
}

impl CModule {
    /// Loads a PyTorch saved JIT model from a file.
    pub fn load<T: AsRef<std::path::Path>>(path: T) -> Fallible<CModule> {
        let path = path_to_cstring(path)?;
        let c_module = unsafe_torch_err!({ atm_load(path.as_ptr()) });
        Ok(CModule { c_module })
    }

    /// Performs the forward pass for a model on some specified tensor inputs.
    pub fn forward_ts<T: Borrow<Tensor>>(&self, ts: &[T]) -> Fallible<Tensor> {
        let ts: Vec<_> = ts.iter().map(|x| x.borrow().c_tensor).collect();
        let c_tensor =
            unsafe_torch_err!({ atm_forward(self.c_module, ts.as_ptr(), ts.len() as c_int) });
        Ok(Tensor { c_tensor })
    }

    /// Performs the forward pass for a model on some specified ivalue input.
    pub fn forward_is<T: Borrow<IValue>>(&self, ts: &[T]) -> Fallible<IValue> {
        let ts = ts
            .iter()
            .map(|x| x.borrow().to_c())
            .collect::<Fallible<Vec<_>>>()?;
        let c_ivalue =
            unsafe_torch_err!({ atm_forward_(self.c_module, ts.as_ptr(), ts.len() as c_int) });
        for x in ts {
            unsafe { ati_free(x) }
        }
        IValue::of_c(c_ivalue)
    }
}

#[cfg(test)]
mod tests {
    use super::IValue;
    #[test]
    fn ivalue() {
        let ivalue = IValue::Tuple(vec![IValue::Int(42), IValue::Double(3.1415)]);
        let ivalue2 = IValue::of_c(ivalue.to_c().unwrap()).unwrap();
        assert_eq!(format!("{:?}", ivalue), format!("{:?}", ivalue2));
    }
}
