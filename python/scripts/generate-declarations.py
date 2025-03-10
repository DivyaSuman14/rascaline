#!/usr/bin/env python
import os

import equistore.core
from pycparser import c_ast, parse_file


ROOT = os.path.dirname(__file__)
FAKE_INCLUDES = os.path.join(ROOT, "include")
EQUISTORE_INCLUDE = os.path.join(
    equistore.core.utils.cmake_prefix_path, "..", "..", "include"
)
RASCALINE_HEADER = os.path.relpath(
    os.path.join(ROOT, "..", "..", "rascaline-c-api", "include", "rascaline.h")
)


class Function:
    def __init__(self, name, restype):
        self.name = name
        self.restype = restype
        self.args = []

    def add_arg(self, arg):
        self.args.append(arg)


class Struct:
    def __init__(self, name):
        self.name = name
        self.members = {}

    def add_member(self, name, type):
        self.members[name] = type


class Enum:
    def __init__(self, name):
        self.name = name
        self.values = {}

    def add_value(self, name, value):
        self.values[name] = value


class AstVisitor(c_ast.NodeVisitor):
    def __init__(self):
        self.functions = []
        self.enums = []
        self.structs = []
        self.types = {}
        self.defines = {}

    def visit_Decl(self, node):
        if not node.name.startswith("rascal_"):
            return

        function = Function(node.name, node.type.type)
        for parameter in node.type.args.params:
            function.add_arg(parameter.type)
        self.functions.append(function)

    def visit_Typedef(self, node):
        if not node.name.startswith("rascal_"):
            return

        if isinstance(node.type.type, c_ast.Enum):
            # Get name and value for enum
            enum = Enum(node.name)
            for enumerator in node.type.type.values.enumerators:
                enum.add_value(enumerator.name, enumerator.value.value)
            self.enums.append(enum)

        elif isinstance(node.type.type, c_ast.Struct):
            struct = Struct(node.name)
            for _, member in node.type.type.children():
                struct.add_member(member.name, member.type)

            self.structs.append(struct)

        else:
            self.types[node.name] = node.type.type


def parse(file):
    cpp_args = ["-E", "-I", FAKE_INCLUDES, "-I", EQUISTORE_INCLUDE]
    ast = parse_file(file, use_cpp=True, cpp_path="gcc", cpp_args=cpp_args)

    visitor = AstVisitor()
    visitor.visit(ast)

    with open(file) as fd:
        for line in fd:
            if "#define" in line:
                split = line.split()
                name = split[1]
                if name == "RASCALINE_H":
                    continue
                value = split[2]

                visitor.defines[name] = value
    return visitor


def c_type_name(name):
    if name.startswith("rascal_"):
        # enums are represented as int
        if name == "rascal_indexes_kind":
            return "ctypes.c_int"
        else:
            return name
    if name.startswith("eqs_"):
        # equistore types
        return name
    elif name == "uintptr_t":
        return "c_uintptr_t"
    elif name == "void":
        return "None"
    elif name == "int32_t":
        return "ctypes.c_int32"
    elif name == "uint32_t":
        return "ctypes.c_uint32"
    elif name == "int64_t":
        return "ctypes.c_int64"
    elif name == "uint64_t":
        return "ctypes.c_uint64"
    else:
        return "ctypes.c_" + name


def _typedecl_name(type):
    assert isinstance(type, c_ast.TypeDecl)
    if isinstance(type.type, c_ast.Struct):
        return type.type.name
    elif isinstance(type.type, c_ast.Enum):
        return type.type.name
    else:
        assert len(type.type.names) == 1
        return type.type.names[0]


def funcdecl_to_ctypes(type, ndpointer=False):
    restype = type_to_ctypes(type.type, ndpointer)
    args = [type_to_ctypes(t.type, ndpointer) for t in type.args.params]

    return f'CFUNCTYPE({restype}, {", ".join(args)})'


def type_to_ctypes(type, ndpointer=False):
    if isinstance(type, c_ast.PtrDecl):
        if isinstance(type.type, c_ast.PtrDecl):
            if isinstance(type.type.type, c_ast.TypeDecl):
                name = _typedecl_name(type.type.type)
                if name == "char":
                    return "POINTER(ctypes.c_char_p)"

                name = c_type_name(name)
                if ndpointer:
                    return f"POINTER(ndpointer({name}, flags='C_CONTIGUOUS'))"
                else:
                    return f"POINTER(POINTER({name}))"

        elif isinstance(type.type, c_ast.TypeDecl):
            name = _typedecl_name(type.type)
            if name == "void":
                return "ctypes.c_void_p"
            elif name == "char":
                return "ctypes.c_char_p"
            else:
                return f"POINTER({c_type_name(name)})"

        elif isinstance(type.type, c_ast.FuncDecl):
            return funcdecl_to_ctypes(type.type, ndpointer)

    else:
        # not a pointer
        if isinstance(type, c_ast.TypeDecl):
            return c_type_name(_typedecl_name(type))
        elif isinstance(type, c_ast.IdentifierType):
            return c_type_name(type.names[0])
        elif isinstance(type, c_ast.ArrayDecl):
            if isinstance(type.dim, c_ast.Constant):
                size = type.dim.value
            else:
                raise Exception("dynamically sized arrays are not supported")

            return f"{type_to_ctypes(type.type)} * {size}"
        elif isinstance(type, c_ast.FuncDecl):
            return funcdecl_to_ctypes(type, ndpointer)

    raise Exception("Unknown type")


def generate_enums(file, enums):
    for enum in enums:
        file.write(f"\n\nclass {enum.name}(enum.Enum):\n")
        for name, value in enum.values.items():
            file.write(f"    {name} = {value}\n")


def generate_structs(file, structs):
    for struct in structs:
        file.write(f"\n\nclass {struct.name}(ctypes.Structure):\n")
        if len(struct.members) == 0:
            file.write("    pass\n")
            continue

        file.write("    _fields_ = [\n")
        for name, type in struct.members.items():
            file.write(f'        ("{name}", {type_to_ctypes(type, True)}),\n')
        file.write("    ]\n")


def generate_functions(file, functions):
    file.write("\n\ndef setup_functions(lib):\n")
    file.write("    from .status import _check_rascal_status_t\n")

    for function in functions:
        file.write(f"\n    lib.{function.name}.argtypes = [\n        ")
        args = [type_to_ctypes(arg) for arg in function.args]

        # functions taking void parameter in C don't have any parameter
        if args == ["None"]:
            args = []
        file.write(",\n        ".join(args))
        file.write("\n    ]\n")

        restype = type_to_ctypes(function.restype)
        if restype == "rascal_status_t":
            restype = "_check_rascal_status_t"

        file.write(f"    lib.{function.name}.restype = {restype}\n")


def generate_declarations():
    data = parse(RASCALINE_HEADER)

    outpath = os.path.join(ROOT, "..", "rascaline", "_c_api.py")
    with open(outpath, "w") as file:
        file.write(
            """'''Automatically-generated file, do not edit!!!'''
# flake8: noqa

import ctypes
import enum
import platform
from ctypes import CFUNCTYPE, POINTER

from equistore.core._c_api import eqs_labels_t, eqs_tensormap_t
from numpy.ctypeslib import ndpointer


arch = platform.architecture()[0]
if arch == "32bit":
    c_uintptr_t = ctypes.c_uint32
elif arch == "64bit":
    c_uintptr_t = ctypes.c_uint64

"""
        )
        for name, value in data.defines.items():
            file.write(f"{name} = {value}\n")
        file.write("\n\n")

        for name, c_type in data.types.items():
            file.write(f"{name} = {type_to_ctypes(c_type)}\n")

        generate_enums(file, data.enums)
        generate_structs(file, data.structs)
        generate_functions(file, data.functions)


if __name__ == "__main__":
    generate_declarations()
