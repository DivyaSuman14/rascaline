use std::collections::{BTreeSet, HashMap};

use ndarray::parallel::prelude::*;

use equistore::{TensorMap, TensorBlock, EmptyArray};
use equistore::{LabelsBuilder, Labels, LabelValue};

use crate::calculators::CalculatorBase;
use crate::{CalculationOptions, Calculator, LabelsSelection};
use crate::{Error, System};

use super::SphericalExpansionParameters;
use super::{SphericalExpansion, CutoffFunction, RadialScaling};
use crate::calculators::radial_basis::RadialBasis;

use crate::labels::{SpeciesFilter, SamplesBuilder};
use crate::labels::AtomCenteredSamples;
use crate::labels::{KeysBuilder, CenterTwoNeighborsSpeciesKeys};


/// Parameters for SOAP power spectrum calculator.
///
/// In the SOAP power spectrum, each sample represents rotationally-averaged
/// atomic density correlations, built on top of the spherical expansion. Each
/// sample is a vector indexed by `n1, n2, l`, where `n1` and `n2` are radial
/// basis indexes and `l` is the angular index:
///
/// `< n1 n2 l | X_i > = \sum_m < n1 l m | X_i > < n2 l m | X_i >`
///
/// where the `< n l m | X_i >` are the spherical expansion coefficients.
///
/// See [this review article](https://doi.org/10.1063/1.5090481) for more
/// information on the SOAP representations.
#[derive(Debug, Clone)]
#[derive(serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct PowerSpectrumParameters {
    /// Spherical cutoff to use for atomic environments
    pub cutoff: f64,
    /// Number of radial basis function to use
    pub max_radial: usize,
    /// Number of spherical harmonics to use
    pub max_angular: usize,
    /// Width of the atom-centered gaussian creating the atomic density
    pub atomic_gaussian_width: f64,
    /// Weight of the central atom contribution to the
    /// features. If `1.0` the center atom contribution is weighted the same
    /// as any other contribution. If `0.0` the central atom does not
    /// contribute to the features at all.
    pub center_atom_weight: f64,
    /// radial basis to use for the radial integral
    pub radial_basis: RadialBasis,
    /// cutoff function used to smooth the behavior around the cutoff radius
    pub cutoff_function: CutoffFunction,
    /// radial scaling can be used to reduce the importance of neighbor atoms
    /// further away from the center, usually improving the performance of the
    /// model
    #[serde(default)]
    pub radial_scaling: RadialScaling,
}

/// Calculator implementing the Smooth Overlap of Atomic Position (SOAP) power
/// spectrum representation of atomistic systems.
pub struct SoapPowerSpectrum {
    parameters: PowerSpectrumParameters,
    spherical_expansion: Calculator,
}

impl std::fmt::Debug for SoapPowerSpectrum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.parameters)
    }
}

impl SoapPowerSpectrum {
    pub fn new(parameters: PowerSpectrumParameters) -> Result<SoapPowerSpectrum, Error> {
        let expansion_parameters = SphericalExpansionParameters {
            cutoff: parameters.cutoff,
            max_radial: parameters.max_radial,
            max_angular: parameters.max_angular,
            atomic_gaussian_width: parameters.atomic_gaussian_width,
            center_atom_weight: parameters.center_atom_weight,
            radial_basis: parameters.radial_basis.clone(),
            cutoff_function: parameters.cutoff_function,
            radial_scaling: parameters.radial_scaling,
        };

        let spherical_expansion = SphericalExpansion::new(expansion_parameters)?;

        return Ok(SoapPowerSpectrum {
            parameters: parameters,
            spherical_expansion: Calculator::from(
                Box::new(spherical_expansion) as Box<dyn CalculatorBase>
            ),
        });
    }

    /// Construct a `TensorMap` containing the set of samples/properties we want
    /// the spherical expansion calculator to compute.
    ///
    /// For each block, samples will contain the same set of samples as the
    /// power spectrum, even if a neighbor species might not be around, since
    /// that simplifies the accumulation loops quite a lot.
    fn selected_spx_labels(&self, descriptor: &TensorMap) -> TensorMap {
        assert_eq!(descriptor.keys().names(), ["species_center", "species_neighbor_1", "species_neighbor_2"]);

        // first, go over the requested power spectrum properties and group them
        // depending on the species_neighbor
        let mut requested_by_key = HashMap::new();
        let mut requested_spherical_harmonics_l = BTreeSet::new();
        for (&[center, neighbor_1, neighbor_2], block) in descriptor.keys().iter_fixed_size().zip(descriptor.blocks()) {
            for &[l, n1, n2] in block.properties().iter_fixed_size() {
                requested_spherical_harmonics_l.insert(l.usize());

                let (_, properties) = requested_by_key.entry([l, center, neighbor_1]).or_insert_with(|| {
                    (BTreeSet::new(), BTreeSet::new())
                });
                properties.insert([n1]);

                let (_, properties) = requested_by_key.entry([l, center, neighbor_2]).or_insert_with(|| {
                    (BTreeSet::new(), BTreeSet::new())
                });
                properties.insert([n2]);
            }
        }

        // make sure all the expected blocks are there, even if the power
        // spectrum does not contain e.g. l=3 at all. The corresponding blocks
        // will have an empty set of properties
        for &[center, neighbor_1, neighbor_2] in descriptor.keys().iter_fixed_size() {
            for &l in &requested_spherical_harmonics_l {
                requested_by_key.entry([l.into(), center, neighbor_1]).or_insert_with(|| {
                    (BTreeSet::new(), BTreeSet::new())
                });

                requested_by_key.entry([l.into(), center, neighbor_2]).or_insert_with(|| {
                    (BTreeSet::new(), BTreeSet::new())
                });
            }
        }

        // Then, loop over the requested power spectrum, and accumulate the
        // samples we want to compute.
        for (&[_, requested_center, requested_neighbor], (samples, _)) in &mut requested_by_key {
            for (key, block) in descriptor {
                let center = key[0];
                let neighbor_1 = key[1];
                let neighbor_2 = key[2];

                if center != requested_center {
                    continue;
                }

                if !(requested_neighbor == neighbor_1 || requested_neighbor == neighbor_2) {
                    continue;
                }

                for &sample in block.samples().iter_fixed_size::<2>() {
                    samples.insert(sample);
                }
            }
        }

        let mut keys_builder = LabelsBuilder::new(vec!["spherical_harmonics_l", "species_center", "species_neighbor"]);
        let mut blocks = Vec::new();
        for (key, (samples, properties)) in requested_by_key {
            keys_builder.add(&key);

            let mut samples_builder = LabelsBuilder::new(vec!["structure", "center"]);
            for entry in samples {
                samples_builder.add(&entry);
            }
            let samples = samples_builder.finish();

            let mut properties_builder = LabelsBuilder::new(vec!["n"]);
            for entry in properties {
                properties_builder.add(&entry);
            }
            let properties = properties_builder.finish();

            blocks.push(TensorBlock::new(
                EmptyArray::new(vec![samples.count(), properties.count()]),
                &samples,
                &[],
                &properties,
            ).expect("invalid TensorBlock"));
        }

        // if the user selected only a subset of l entries, make sure there are
        // empty blocks for the corresponding keys in the spherical expansion
        // selection
        let mut missing_keys = BTreeSet::new();
        for &[center, neighbor_1, neighbor_2] in descriptor.keys().iter_fixed_size() {
            for spherical_harmonics_l in 0..=(self.parameters.max_angular) {
                if !requested_spherical_harmonics_l.contains(&spherical_harmonics_l) {
                    missing_keys.insert([spherical_harmonics_l.into(), center, neighbor_1]);
                    missing_keys.insert([spherical_harmonics_l.into(), center, neighbor_2]);
                }
            }
        }
        for key in missing_keys {
            keys_builder.add(&key);

            let samples = Labels::empty(vec!["structure", "center"]);
            let properties = Labels::empty(vec!["n"]);
            blocks.push(TensorBlock::new(
                EmptyArray::new(vec![samples.count(), properties.count()]),
                &samples,
                &[],
                &properties,
            ).expect("invalid TensorBlock"));
        }

        return TensorMap::new(keys_builder.finish(), blocks).expect("invalid TensorMap")
    }

    /// Pre-compute the correspondance between samples of the spherical
    /// expansion & the power spectrum, both for values and gradients.
    ///
    /// For example, the key `center, neighbor_1, neighbor_2 = 1, 6, 8` will
    /// have a very different set of samples from `c, n_1, n_2 = 1, 6, 6`; but
    /// both will use the spherical expansion `center, neighbor = 1, 6`.
    ///
    /// This function returns the list of spherical expansion sample indexes
    /// corresponding to the requested samples in `descriptor` for each block.
    fn samples_mapping(
        descriptor: &TensorMap,
        spherical_expansion: &TensorMap
    ) -> HashMap<Vec<LabelValue>, SamplesMapping> {
        let mut mapping = HashMap::new();
        for (key, block) in descriptor.iter() {
            let species_center = key[0];
            let species_neighbor_1 = key[1];
            let species_neighbor_2 = key[2];

            let block_data = block.data();
            if block_data.properties.count() == 0 {
                // no properties to compute, we don't really care about sample
                // mapping and we can not compute the real one (there is no l to
                // find the corresponding spx block), so we'll create a dummy
                // sample mapping / gradient sample mapping
                let mut values_mapping = Vec::new();
                for i in 0..block_data.samples.count() {
                    values_mapping.push((i, i));
                }

                let mut gradient_mapping = Vec::new();
                if let Some(gradient) = block.gradient("positions") {
                    let gradient = gradient.data();
                    for i in 0..gradient.samples.count() {
                        gradient_mapping.push((Some(i), Some(i)));
                    }
                }

                mapping.insert(key.to_vec(), SamplesMapping {
                    values: values_mapping,
                    gradients: gradient_mapping,
                });
                continue;
            }

            let mut values_mapping = Vec::new();

            // the spherical expansion samples are the same for all
            // `spherical_harmonics_l` values, so we only need to compute it for
            // the first one.
            let first_l = block_data.properties[0][0];

            let block_id_1 = spherical_expansion.keys().position(&[
                first_l, species_center, species_neighbor_1
            ]).expect("missing block in spherical expansion");
            let spx_block_1 = &spherical_expansion.block_by_id(block_id_1);
            let spx_samples_1 = spx_block_1.samples();

            let block_id_2 = spherical_expansion.keys().position(&[
                first_l, species_center, species_neighbor_2
            ]).expect("missing block in spherical expansion");
            let spx_block_2 = &spherical_expansion.block_by_id(block_id_2);
            let spx_samples_2 = spx_block_2.samples();

            values_mapping.reserve(block_data.samples.count());
            for sample in &*block_data.samples {
                let sample_1 = spx_samples_1.position(sample).expect("missing spherical expansion sample");
                let sample_2 = spx_samples_2.position(sample).expect("missing spherical expansion sample");
                values_mapping.push((sample_1, sample_2));
            }

            let mut gradient_mapping = Vec::new();
            if let Some(gradient) = block.gradient("positions") {
                let spx_gradient_1 = spx_block_1.gradient("positions").expect("missing spherical expansion gradients");
                let spx_gradient_2 = spx_block_2.gradient("positions").expect("missing spherical expansion gradients");

                let gradient_samples = gradient.samples();
                gradient_mapping.reserve(gradient_samples.count());

                let spx_gradient_1_samples = spx_gradient_1.samples();
                let spx_gradient_2_samples = spx_gradient_2.samples();

                for gradient_sample in gradient_samples.iter() {
                    gradient_mapping.push((
                        spx_gradient_1_samples.position(gradient_sample),
                        spx_gradient_2_samples.position(gradient_sample),
                    ));
                }
            }

            mapping.insert(key.to_vec(), SamplesMapping {
                values: values_mapping,
                gradients: gradient_mapping
            });
        }

        return mapping;
    }

    /// Get the list of spherical expansion to combine when computing a single
    /// block (associated with the given key) of the power spectrum.
    fn spx_properties_to_combine<'a>(
        key: &[LabelValue],
        properties: &Labels,
        spherical_expansion: &HashMap<&[LabelValue], SphericalExpansionBlock<'a>>,
    ) -> Vec<SpxPropertiesToCombine<'a>> {
        let species_center = key[0];
        let species_neighbor_1 = key[1];
        let species_neighbor_2 = key[2];

        return properties.par_iter().map(|property| {
            let l = property[0];
            let n1 = property[1];
            let n2 = property[2];

            let key_1: &[_] = &[l, species_center, species_neighbor_1];
            let block_1 = spherical_expansion.get(&key_1)
            .expect("missing first neighbor species block in spherical expansion");

            let key_2: &[_] = &[l, species_center, species_neighbor_2];
            let block_2 = spherical_expansion.get(&key_2)
                .expect("missing first neighbor species block in spherical expansion");

            // both blocks should had the same number of m components
            debug_assert_eq!(block_1.values.shape()[1], block_2.values.shape()[1]);

            let property_1 = block_1.properties.position(&[n1]).expect("missing n1");
            let property_2 = block_2.properties.position(&[n2]).expect("missing n2");

            SpxPropertiesToCombine {
                spherical_harmonics_l: l.usize(),
                property_1,
                property_2,
                spx_1: block_1.clone(),
                spx_2: block_2.clone(),
            }
        }).collect();
    }
}


/// Data about the two spherical expansion block that will get combined to
/// produce a single (l, n1, n2) property in a single power spectrum block
struct SpxPropertiesToCombine<'a> {
    /// value of l
    spherical_harmonics_l: usize,
    /// position of n1 in the first spherical expansion properties
    property_1: usize,
    /// position of n2 in the second spherical expansion properties
    property_2: usize,
    /// first spherical expansion block
    spx_1: SphericalExpansionBlock<'a>,
    /// second spherical expansion block
    spx_2: SphericalExpansionBlock<'a>,
}

/// Data from a single spherical expansion block
#[derive(Debug, Clone)]
struct SphericalExpansionBlock<'a> {
    properties: Labels,
    /// spherical expansion values
    values: &'a ndarray::ArrayD<f64>,
    /// spherical expansion position gradients
    positions_gradients: Option<&'a ndarray::ArrayD<f64>>,
    /// spherical expansion cell gradients
    cell_gradients: Option<&'a ndarray::ArrayD<f64>>,
}

/// Indexes of the spherical expansion samples/rows corresponding to each power
/// spectrum row.
struct SamplesMapping {
    /// Mapping for the values.
    values: Vec<(usize, usize)>,
    /// Mapping for the gradients.
    ///
    /// Some samples might not be defined in both of the spherical expansion
    /// blocks being considered, for examples when dealing with two different
    /// neighbor species, only one the sample corresponding to the right
    /// neighbor species will be `Some`.
    gradients: Vec<(Option<usize>, Option<usize>)>,
}

impl CalculatorBase for SoapPowerSpectrum {
    fn name(&self) -> String {
        "SOAP power spectrum".into()
    }

    fn parameters(&self) -> String {
        serde_json::to_string(&self.parameters).expect("failed to serialize to JSON")
    }

    fn keys(&self, systems: &mut [Box<dyn System>]) -> Result<equistore::Labels, Error> {
        let builder = CenterTwoNeighborsSpeciesKeys {
            cutoff: self.parameters.cutoff,
            self_pairs: true,
            symmetric: true,
        };
        return builder.keys(systems);
    }

    fn samples_names(&self) -> Vec<&str> {
        AtomCenteredSamples::samples_names()
    }

    fn samples(&self, keys: &equistore::Labels, systems: &mut [Box<dyn System>]) -> Result<Vec<Labels>, Error> {
        assert_eq!(keys.names(), ["species_center", "species_neighbor_1", "species_neighbor_2"]);
        let mut result = Vec::new();
        for [species_center, species_neighbor_1, species_neighbor_2] in keys.iter_fixed_size() {

            let builder = AtomCenteredSamples {
                cutoff: self.parameters.cutoff,
                species_center: SpeciesFilter::Single(species_center.i32()),
                // we only want center with both neighbor species present
                species_neighbor: SpeciesFilter::AllOf(
                    [
                        species_neighbor_1.i32(),
                        species_neighbor_2.i32()
                    ].iter().copied().collect()
                ),
                self_pairs: true,
            };

            result.push(builder.samples(systems)?);
        }

        return Ok(result);
    }

    fn positions_gradient_samples(&self, keys: &Labels, samples: &[Labels], systems: &mut [Box<dyn System>]) -> Result<Vec<Labels>, Error> {
        assert_eq!(keys.names(), ["species_center", "species_neighbor_1", "species_neighbor_2"]);
        assert_eq!(keys.count(), samples.len());

        let mut gradient_samples = Vec::new();
        for ([species_center, species_neighbor_1, species_neighbor_2], samples) in keys.iter_fixed_size().zip(samples) {
            let builder = AtomCenteredSamples {
                cutoff: self.parameters.cutoff,
                species_center: SpeciesFilter::Single(species_center.i32()),
                // gradients samples should contain either neighbor species
                species_neighbor: SpeciesFilter::OneOf(vec![
                    species_neighbor_1.i32(),
                    species_neighbor_2.i32()
                ]),
                self_pairs: true,
            };

            gradient_samples.push(builder.gradients_for(systems, samples)?);
        }

        return Ok(gradient_samples);
    }

    fn supports_gradient(&self, parameter: &str) -> bool {
        match parameter {
            "positions" => true,
            "cell" => true,
            _ => false,
        }
    }

    fn components(&self, keys: &equistore::Labels) -> Vec<Vec<Labels>> {
        return vec![vec![]; keys.count()];
    }

    fn properties_names(&self) -> Vec<&str> {
        vec!["l", "n1", "n2"]
    }

    fn properties(&self, keys: &equistore::Labels) -> Vec<Labels> {
        let mut properties = LabelsBuilder::new(self.properties_names());
        for l in 0..=self.parameters.max_angular {
            for n1 in 0..self.parameters.max_radial {
                for n2 in 0..self.parameters.max_radial {
                    properties.add(&[l, n1, n2]);
                }
            }
        }
        let properties = properties.finish();

        return vec![properties; keys.count()];
    }

    #[time_graph::instrument(name = "SoapPowerSpectrum::compute")]
    #[allow(clippy::too_many_lines)]
    fn compute(&mut self, systems: &mut [Box<dyn System>], descriptor: &mut TensorMap) -> Result<(), Error> {
        let mut gradients = Vec::new();
        if descriptor.block_by_id(0).gradient("positions").is_some() {
            gradients.push("positions");
        }
        if descriptor.block_by_id(0).gradient("cell").is_some() {
            gradients.push("cell");
        }

        let selected = self.selected_spx_labels(descriptor);

        let options = CalculationOptions {
            gradients: &gradients,
            selected_samples: LabelsSelection::Predefined(&selected),
            selected_properties: LabelsSelection::Predefined(&selected),
            selected_keys: Some(selected.keys()),
            ..Default::default()
        };

        let spherical_expansion = self.spherical_expansion.compute(
            systems,
            options,
        ).expect("failed to compute spherical expansion");
        let samples_mapping = SoapPowerSpectrum::samples_mapping(descriptor, &spherical_expansion);

        let spherical_expansion = spherical_expansion.iter().map(|(key, block)| {
            let spx_block = SphericalExpansionBlock {
                properties: block.properties(),
                values: block.values().to_array(),
                positions_gradients: block.gradient("positions").map(|g| g.values().to_array()),
                cell_gradients: block.gradient("cell").map(|g| g.values().to_array()),
            };

            (key, spx_block)
        }).collect();

        for (key, mut block) in descriptor.iter_mut() {
            let species_neighbor_1 = key[1];
            let species_neighbor_2 = key[2];

            let mut block_data = block.data_mut();
            let properties_to_combine = SoapPowerSpectrum::spx_properties_to_combine(
                key,
                &block_data.properties,
                &spherical_expansion,
            );

            let mapping = samples_mapping.get(key).expect("missing sample mapping");

            block_data.values.as_array_mut()
                .axis_iter_mut(ndarray::Axis(0))
                .into_par_iter()
                .zip_eq(&mapping.values)
                .for_each(|(mut values, &(spx_sample_1, spx_sample_2))| {
                    for (property_i, spx) in properties_to_combine.iter().enumerate() {
                        let SpxPropertiesToCombine { spx_1, spx_2, ..} = spx;

                        let mut sum = 0.0;

                        for m in 0..(2 * spx.spherical_harmonics_l + 1) {
                            // unsafe is required to remove the bound checking
                            // in release mode (`uget` still checks bounds in
                            // debug mode)
                            unsafe {
                                sum += spx_1.values.uget([spx_sample_1, m, spx.property_1])
                                     * spx_2.values.uget([spx_sample_2, m, spx.property_2]);
                            }
                        }

                        if species_neighbor_1 != species_neighbor_2 {
                            // We only store values for `species_neighbor_1 <
                            // species_neighbor_2` because the values are the
                            // same for pairs `species_neighbor_1 <->
                            // species_neighbor_2` and `species_neighbor_2 <->
                            // species_neighbor_1`. To ensure the final kernels
                            // are correct, we have to multiply the
                            // corresponding values.
                            sum *= std::f64::consts::SQRT_2;
                        }

                        unsafe {
                            *values.uget_mut(property_i) = sum / f64::sqrt((2 * spx.spherical_harmonics_l + 1) as f64);
                        }
                    }
                });

            // gradients with respect to the atomic positions
            if let Some(mut gradient) = block.gradient_mut("positions") {
                let gradient = gradient.data_mut();

                gradient.values.to_array_mut()
                    .axis_iter_mut(ndarray::Axis(0))
                    .into_par_iter()
                    .zip_eq(gradient.samples.par_iter())
                    .zip_eq(&mapping.gradients)
                    .for_each(|((mut values, gradient_sample), &(spx_grad_sample_1, spx_grad_sample_2))| {
                        for (property_i, spx) in properties_to_combine.iter().enumerate() {
                            let SpxPropertiesToCombine { spx_1, spx_2, ..} = spx;

                            let spx_1_gradient = spx_1.positions_gradients.expect("missing spherical expansion gradients");
                            let spx_2_gradient = spx_2.positions_gradients.expect("missing spherical expansion gradients");

                            let sample_i = gradient_sample[0].usize();
                            let (spx_sample_1, spx_sample_2) = mapping.values[sample_i];

                            let mut sum = [0.0, 0.0, 0.0];
                            if let Some(grad_sample_1) = spx_grad_sample_1 {
                                for m in 0..(2 * spx.spherical_harmonics_l + 1) {
                                    // SAFETY: see same loop for values
                                    unsafe {
                                        let value_2 = spx_2.values.uget([spx_sample_2, m, spx.property_2]);
                                        for d in 0..3 {
                                            sum[d] += value_2 * spx_1_gradient.uget([grad_sample_1, d, m, spx.property_1]);
                                        }
                                    }
                                }
                            }

                            if let Some(grad_sample_2) = spx_grad_sample_2 {
                                for m in 0..(2 * spx.spherical_harmonics_l + 1) {
                                    // SAFETY: see same loop for values
                                    unsafe {
                                        let value_1 = spx_1.values.uget([spx_sample_1, m, spx.property_1]);
                                        for d in 0..3 {
                                            sum[d] += value_1 * spx_2_gradient.uget([grad_sample_2, d, m, spx.property_2]);
                                        }
                                    }
                                }
                            }

                            if species_neighbor_1 != species_neighbor_2 {
                                // see above
                                for d in 0..3 {
                                    sum[d] *= std::f64::consts::SQRT_2;
                                }
                            }

                            let normalization = f64::sqrt((2 * spx.spherical_harmonics_l + 1) as f64);
                            for d in 0..3 {
                                unsafe {
                                    *values.uget_mut([d, property_i]) = sum[d] / normalization;
                                }
                            }
                        }
                    });
            }

            // gradients with respect to the cell parameters
            if let Some(mut gradient) = block.gradient_mut("cell") {
                let gradient = gradient.data_mut();

                gradient.values.to_array_mut()
                    .axis_iter_mut(ndarray::Axis(0))
                    .into_par_iter()
                    .zip_eq(gradient.samples.par_iter())
                    .for_each(|(mut values, gradient_sample)| {
                        for (property_i, spx) in properties_to_combine.iter().enumerate() {
                            let SpxPropertiesToCombine { spx_1, spx_2, ..} = spx;

                            let spx_1_gradient = spx_1.cell_gradients.expect("missing spherical expansion gradients");
                            let spx_2_gradient = spx_2.cell_gradients.expect("missing spherical expansion gradients");

                            let sample_i = gradient_sample[0].usize();
                            let (spx_sample_1, spx_sample_2) = mapping.values[sample_i];

                            let mut sum = [
                                [0.0, 0.0, 0.0],
                                [0.0, 0.0, 0.0],
                                [0.0, 0.0, 0.0],
                            ];
                            for m in 0..(2 * spx.spherical_harmonics_l + 1) {
                                // SAFETY: see same loop for values
                                unsafe {
                                    let value_2 = spx_2.values.uget([spx_sample_2, m, spx.property_2]);
                                    for d1 in 0..3 {
                                        for d2 in 0..3 {
                                            // TODO: ensure that gradient samples are 0..nsamples
                                            sum[d1][d2] += value_2 * spx_1_gradient.uget([spx_sample_1, d1, d2, m, spx.property_1]);
                                        }
                                    }
                                }
                            }

                            for m in 0..(2 * spx.spherical_harmonics_l + 1) {
                                // SAFETY: see same loop for values
                                unsafe {
                                    let value_1 = spx_1.values.uget([spx_sample_1, m, spx.property_1]);
                                    for d1 in 0..3 {
                                        for d2 in 0..3 {
                                            // TODO: ensure that gradient samples are 0..nsamples
                                            sum[d1][d2] += value_1 * spx_2_gradient.uget([spx_sample_2, d1, d2, m, spx.property_2]);
                                        }
                                    }
                                }
                            }

                            if species_neighbor_1 != species_neighbor_2 {
                                // see above
                                for d1 in 0..3 {
                                    for d2 in 0..3 {
                                        sum[d1][d2] *= std::f64::consts::SQRT_2;
                                    }
                                }
                            }

                            let normalization = f64::sqrt((2 * spx.spherical_harmonics_l + 1) as f64);

                            for d1 in 0..3 {
                                for d2 in 0..3 {
                                    unsafe {
                                        *values.uget_mut([d1, d2, property_i]) = sum[d1][d2] / normalization;
                                    }
                                }
                            }
                        }
                    });
            }

        }

        Ok(())
    }
}


#[cfg(test)]
mod tests {
    use equistore::LabelValue;

    use crate::systems::test_utils::{test_systems, test_system};
    use crate::Calculator;

    use super::*;
    use crate::calculators::CalculatorBase;

    fn parameters() -> PowerSpectrumParameters {
        PowerSpectrumParameters {
            cutoff: 3.5,
            max_radial: 6,
            max_angular: 6,
            atomic_gaussian_width: 0.3,
            center_atom_weight: 1.0,
            radial_basis: RadialBasis::splined_gto(1e-8),
            radial_scaling: RadialScaling::None {},
            cutoff_function: CutoffFunction::ShiftedCosine { width: 0.5 },
        }
    }

    #[test]
    fn values() {
        let mut calculator = Calculator::from(Box::new(SoapPowerSpectrum::new(
            parameters()
        ).unwrap()) as Box<dyn CalculatorBase>);

        let mut systems = test_systems(&["water"]);
        let descriptor = calculator.compute(&mut systems, Default::default()).unwrap();

        assert_eq!(descriptor.keys().count(), 6);
        assert!(descriptor.keys().contains(
            &[LabelValue::new(1), LabelValue::new(1), LabelValue::new(1)]
        ));
        assert!(descriptor.keys().contains(
            &[LabelValue::new(1), LabelValue::new(-42), LabelValue::new(1)]
        ));
        assert!(descriptor.keys().contains(
            &[LabelValue::new(1), LabelValue::new(-42), LabelValue::new(-42)]
        ));

        assert!(descriptor.keys().contains(
            &[LabelValue::new(-42), LabelValue::new(1), LabelValue::new(1)]
        ));
        assert!(descriptor.keys().contains(
            &[LabelValue::new(-42), LabelValue::new(-42), LabelValue::new(1)]
        ));
        assert!(descriptor.keys().contains(
            &[LabelValue::new(-42), LabelValue::new(-42), LabelValue::new(-42)]
        ));

        // exact values for power spectrum are regression-tested in
        // `rascaline/tests/soap-power-spectrum.rs`
    }

    #[test]
    fn finite_differences_positions() {
        let calculator = Calculator::from(Box::new(SoapPowerSpectrum::new(
            parameters()
        ).unwrap()) as Box<dyn CalculatorBase>);

        let system = test_system("water");
        let options = crate::calculators::tests_utils::FinalDifferenceOptions {
            displacement: 1e-6,
            max_relative: 5e-5,
            epsilon: 1e-16,
        };
        crate::calculators::tests_utils::finite_differences_positions(calculator, &system, options);
    }

    #[test]
    fn finite_differences_cell() {
        let calculator = Calculator::from(Box::new(SoapPowerSpectrum::new(
            parameters()
        ).unwrap()) as Box<dyn CalculatorBase>);

        let system = test_system("water");
        let options = crate::calculators::tests_utils::FinalDifferenceOptions {
            displacement: 1e-5,
            max_relative: 1e-5,
            epsilon: 1e-16,
        };
        crate::calculators::tests_utils::finite_differences_cell(calculator, &system, options);
    }

    #[test]
    fn compute_partial() {
        let calculator = Calculator::from(Box::new(SoapPowerSpectrum::new(
            parameters()
        ).unwrap()) as Box<dyn CalculatorBase>);

        let mut systems = test_systems(&["methane"]);

        let properties = Labels::new(["l", "n1", "n2"], &[
            [0, 0, 1],
            [3, 3, 3],
            [2, 4, 3],
            [1, 4, 4],
            [5, 1, 0],
            [1, 1, 2],
        ]);

        let samples = Labels::new(["structure", "center"], &[
            [0, 2],
            [0, 1],
        ]);

        let keys = Labels::new(["species_center", "species_neighbor_1", "species_neighbor_2"], &[
            [1, 1, 1],
            [6, 6, 6],
            [1, 8, 6], // not part of the default keys
            [1, 6, 6],
            [1, 1, 6],
            [6, 1, 1],
            [6, 1, 6],
        ]);

        crate::calculators::tests_utils::compute_partial(
            calculator, &mut systems, &keys, &samples, &properties
        );
    }

    #[test]
    fn compute_partial_per_key() {
        let keys = Labels::new(["species_center", "species_neighbor_1", "species_neighbor_2"], &[
            [1, 1, 1],
            [1, 1, 6],
            [1, 6, 6],
            [6, 1, 1],
            [6, 1, 6],
            [6, 6, 6],
        ]);

        let empty_block = equistore::TensorBlock::new(
            EmptyArray::new(vec![1, 0]),
            &Labels::single(),
            &[],
            &Labels::new::<i32, 3>(["l", "n1", "n2"], &[]),
        ).unwrap();

        let blocks = vec![
            // H, H-H
            equistore::TensorBlock::new(
                EmptyArray::new(vec![1, 1]),
                &Labels::single(),
                &[],
                &Labels::new(["l", "n1", "n2"], &[[2, 0, 0]]),
            ).unwrap(),
            // H, C-H
            empty_block.as_ref().try_clone().unwrap(),
            // H, C-C
            empty_block.as_ref().try_clone().unwrap(),
            // C, H-H
            empty_block.as_ref().try_clone().unwrap(),
            // C, C-H
            equistore::TensorBlock::new(
                EmptyArray::new(vec![1, 1]),
                &Labels::single(),
                &[],
                &Labels::new(["l", "n1", "n2"], &[[3, 0, 0]]),
            ).unwrap(),
            // C, C-C
            empty_block,
        ];
        let selection = equistore::TensorMap::new(keys, blocks).unwrap();

        let options = CalculationOptions {
            selected_properties: LabelsSelection::Predefined(&selection),
            ..Default::default()
        };

        let mut calculator = Calculator::from(Box::new(SoapPowerSpectrum::new(
            parameters()
        ).unwrap()) as Box<dyn CalculatorBase>);

        let mut systems = test_systems(&["methane"]);
        let descriptor = calculator.compute(&mut systems, options).unwrap();


        assert_eq!(descriptor.keys(), selection.keys());

        assert_eq!(descriptor.block_by_id(0).values().as_array().shape(), [4, 1]);
        assert_eq!(descriptor.block_by_id(1).values().as_array().shape(), [4, 0]);
        assert_eq!(descriptor.block_by_id(2).values().as_array().shape(), [4, 0]);
        assert_eq!(descriptor.block_by_id(3).values().as_array().shape(), [1, 0]);
        assert_eq!(descriptor.block_by_id(4).values().as_array().shape(), [1, 1]);
        assert_eq!(descriptor.block_by_id(5).values().as_array().shape(), [1, 0]);
    }

    #[test]
    fn center_atom_weight() {
        let system = &mut test_systems(&["CH"]);

        let mut parameters = parameters();
        parameters.cutoff = 0.5;
        parameters.center_atom_weight = 1.0;

        let mut calculator = Calculator::from(Box::new(
            SoapPowerSpectrum::new(parameters.clone()).unwrap(),
        ) as Box<dyn CalculatorBase>);
        let descriptor = calculator.compute(system, Default::default()).unwrap();

        parameters.center_atom_weight = 0.5;
        let mut calculator = Calculator::from(Box::new(
            SoapPowerSpectrum::new(parameters).unwrap(),
        ) as Box<dyn CalculatorBase>);

        let descriptor_scaled = calculator.compute(system, Default::default()).unwrap();

        for (block, block_scaled) in descriptor.blocks().iter().zip(descriptor_scaled.blocks()) {
            assert_eq!(block.values().as_array(), 4.0 * block_scaled.values().as_array());
        }
    }
}
