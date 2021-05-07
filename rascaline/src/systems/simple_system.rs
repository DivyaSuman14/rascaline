use super::{UnitCell, System, Vector3D, Pair};

use super::neighbors::NeighborsList;

/// A simple implementation of `System` to use when no other is available
#[derive(Clone, Debug)]
pub struct SimpleSystem {
    cell: UnitCell,
    species: Vec<usize>,
    positions: Vec<Vector3D>,
    neighbors: Option<NeighborsList>,
}

impl SimpleSystem {
    /// Create a new empty system with the given unit cell
    pub fn new(cell: UnitCell) -> SimpleSystem {
        SimpleSystem {
            cell: cell,
            species: Vec::new(),
            positions: Vec::new(),
            neighbors: None,
        }
    }

    /// Add an atom with the given species and position to this system
    pub fn add_atom(&mut self, species: usize, position: Vector3D) {
        self.species.push(species);
        self.positions.push(position);
    }

    #[cfg(test)]
    pub(crate) fn positions_mut(&mut self) -> &mut [Vector3D] {
        // any position access invalidates the neighbor list
        self.neighbors = None;
        return &mut self.positions;
    }
}

impl System for SimpleSystem {
    fn size(&self) -> usize {
        self.species.len()
    }

    fn positions(&self) -> &[Vector3D] {
        &self.positions
    }

    fn species(&self) -> &[usize] {
        &self.species
    }

    fn cell(&self) -> UnitCell {
        self.cell
    }

    #[allow(clippy::float_cmp)]
    fn compute_neighbors(&mut self, cutoff: f64) {
        // re-use already computed NL is possible
        if let Some(ref nl) = self.neighbors {
            if nl.cutoff == cutoff {
                return;
            }
        }

        self.neighbors = Some(NeighborsList::new(self.positions(), self.cell(), cutoff));
    }

    fn pairs(&self) -> &[Pair] {
        &self.neighbors.as_ref().expect("neighbor list is not initialized").pairs
    }

    fn pairs_containing(&self, center: usize) -> &[Pair] {
        &self.neighbors.as_ref().expect("neighbor list is not initialized").pairs_by_center[center]
    }
}

impl From<&dyn System> for SimpleSystem {
    fn from(system: &dyn System) -> SimpleSystem {
        let mut new = SimpleSystem::new(system.cell());
        for (&species, &position) in system.species().iter().zip(system.positions()) {
            new.add_atom(species, position);
        }
        return new;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_atoms() {
        let mut system = SimpleSystem::new(UnitCell::cubic(10.0));
        system.add_atom(3, Vector3D::new(2.0, 3.0, 4.0));
        system.add_atom(1, Vector3D::new(1.0, 3.0, 4.0));
        system.add_atom(3, Vector3D::new(5.0, 3.0, 4.0));

        assert_eq!(system.size(), 3);
        assert_eq!(system.species.len(), 3);
        assert_eq!(system.positions.len(), 3);

        assert_eq!(system.species(), &[3, 1, 3]);
        assert_eq!(system.positions(), &[
            Vector3D::new(2.0, 3.0, 4.0),
            Vector3D::new(1.0, 3.0, 4.0),
            Vector3D::new(5.0, 3.0, 4.0),
        ]);
    }
}
