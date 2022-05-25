use std::sync::Arc;

use equistore::{TensorMap, Labels};

use crate::{Error, System};

/// The `CalculatorBase` trait is the interface shared by all calculator
/// implementations; and used by [`crate::Calculator`] to run the calculation.
///
/// This should not be used directly by end users, who should use the facilities
/// in [`crate::Calculator`] instead.
///
/// `std::panic::RefUnwindSafe` is a required super-trait to enable passing
/// calculators across the C API.
pub trait CalculatorBase: std::panic::RefUnwindSafe {
    /// Get the name of this Calculator
    fn name(&self) -> String;

    /// Get the parameters used to create this Calculator as a JSON string
    fn parameters(&self) -> String;

    /// Get the set of keys for this calculator and the given systems
    fn keys(&self, systems: &mut [Box<dyn System>]) -> Result<Labels, Error>;

    /// Get the names used for sample labels by this calculator
    fn samples_names(&self) -> Vec<&str>;

    /// Get the full list of samples this calculator would create for the given
    /// systems. This function should return one set of samples for each key.
    fn samples(&self, keys: &Labels, systems: &mut [Box<dyn System>]) -> Result<Vec<Arc<Labels>>, Error>;

    /// Get the gradient samples corresponding the given samples. The samples
    /// slice contains one set of samples for each key.
    ///
    /// This function should return `None` if no gradients are to be computed,
    /// or `Some(Vec<>)`, with one set of gradient samples for each key.
    fn gradient_samples(&self, keys: &Labels, samples: &[Arc<Labels>], systems: &mut [Box<dyn System>]) -> Result<Option<Vec<Arc<Labels>>>, Error>;

    /// Get the components this calculator computes for each key.
    fn components(&self, keys: &Labels) -> Vec<Vec<Arc<Labels>>>;

    /// Get the names used for property labels by this calculator
    fn properties_names(&self) -> Vec<&str>;

    /// Get the properties this calculator computes for each key.
    fn properties(&self, keys: &Labels) -> Vec<Arc<Labels>>;

    /// Actually run the calculation.
    ///
    /// This function is given a pre-allocated descriptor, filled with zeros.
    /// The samples/properties in each blocks might not match the values
    /// returned by [`CalculatorBase::samples`] and
    /// [`CalculatorBase::properties`]: instead they will only contain the
    /// values that where requested by the end user.
    ///
    /// Gradients are allocated if [`CalculatorBase::gradient_samples`] returned
    /// `Some`.
    fn compute(&mut self, systems: &mut [Box<dyn System>], descriptor: &mut TensorMap) -> Result<(), Error>;
}


#[cfg(test)]
pub(crate) mod tests_utils;

mod dummy_calculator;
pub use self::dummy_calculator::DummyCalculator;

mod sorted_distances;
pub use self::sorted_distances::SortedDistances;

pub mod soap;
pub use self::soap::{SphericalExpansion, SphericalExpansionParameters};
pub use self::soap::{SoapPowerSpectrum, PowerSpectrumParameters};
