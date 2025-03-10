#include <iostream>
#include <rascaline.hpp>

int main(int argc, char* argv[]) {
    if (argc < 2) {
        std::cout << "error: expected a command line argument" << std::endl;
        return 1;
    }
    auto systems = rascaline::BasicSystems(argv[1]);

    // pass hyper-parameters as JSON
    const char* parameters = R"({
        "cutoff": 5.0,
        "max_radial": 6,
        "max_angular": 4,
        "atomic_gaussian_width": 0.3,
        "center_atom_weight": 1.0,
        "gradients": false,
        "radial_basis": {
            "Gto": {}
        },
        "cutoff_function": {
            "ShiftedCosine": {"width": 0.5}
        }
    })";

    // create the calculator with its name and parameters
    auto calculator = rascaline::Calculator("soap_power_spectrum", parameters);

    // run the calculation
    auto descriptor = calculator.compute(systems);

    // The descriptor is an equistore `TensorMap`, containing multiple blocks.
    // We can transform it to a single block containing a dense representation,
    // with one sample for each atom-centered environment.
    descriptor.keys_to_samples("species_center");
    descriptor.keys_to_properties(std::vector<std::string>{"species_neighbor_1", "species_neighbor_2"});

    // extract values from the descriptor in the only remaining block
    auto values = descriptor.block_by_id(0).values();

    // you can now use values as the input of a machine learning algorithm

    return 0;
}
