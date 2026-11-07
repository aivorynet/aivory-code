//! Per-language tests with small synthetic sources: declarations, imports, a syntax error,
//! folding, and highlight.

use DeclKind::*;
use Language as L;
use TokenKind as T;
use Visibility::*;
use aivory_syntax::{
    DeclKind, Declaration, Document, Language, Stub, SyntaxErrorKind, TokenKind, Visibility,
    highlight, syntax_errors,
};

struct Case {
    lang: Language,
    source: &'static str,
    /// (name, kind, container). Every entry must exist.
    decls: &'static [(&'static str, DeclKind, Option<&'static str>)],
    /// (path, alias). Every entry must exist.
    imports: &'static [(&'static str, Option<&'static str>)],
    /// A broken variant of the source: it must give a syntax error.
    broken: &'static str,
    /// (start line, end line) of a fold that must exist.
    fold: (u32, u32),
    /// Token kinds that the highlight must contain.
    tokens: &'static [TokenKind],
}

fn find<'a>(stub: &'a Stub, name: &str, kind: DeclKind) -> &'a Declaration {
    stub.declarations
        .iter()
        .find(|d| d.name == name && d.kind == kind)
        .unwrap_or_else(|| {
            let all: Vec<_> = stub
                .declarations
                .iter()
                .map(|d| format!("{:?} {} in {:?}", d.kind, d.name, d.container))
                .collect();
            panic!(
                "no {kind:?} named {name:?} in {:?}: {all:#?}",
                stub.language
            )
        })
}

fn run(case: &Case) {
    let lang = case.lang;
    let stub = Stub::parse(lang, case.source).unwrap();
    assert_eq!(stub.language, lang);
    for &(name, kind, container) in case.decls {
        let d = find(&stub, name, kind);
        if let Some(c) = container {
            assert_eq!(
                d.container.as_deref(),
                Some(c),
                "container of {name} in {lang:?}"
            );
        }
        assert_eq!(
            &case.source[d.name_range.bytes()],
            name.trim(),
            "name range of {name}"
        );
        assert!(
            d.range.contains(&d.name_range),
            "{name}: range holds name range"
        );
        assert!(!d.signature.is_empty(), "{name}: signature in {lang:?}");
    }
    for &(path, alias) in case.imports {
        let i = stub
            .imports
            .iter()
            .find(|i| i.path == path)
            .unwrap_or_else(|| panic!("no import {path:?} in {lang:?}: {:#?}", stub.imports));
        assert_eq!(
            i.alias.as_deref(),
            alias,
            "alias of import {path} in {lang:?}"
        );
    }
    // The clean source has no syntax error. The broken source has one.
    let clean = syntax_errors(lang, case.source).unwrap();
    assert!(clean.is_empty(), "{lang:?} clean source: {clean:#?}");
    let errors = syntax_errors(lang, case.broken).unwrap();
    assert!(
        !errors.is_empty(),
        "{lang:?}: no error in the broken source"
    );
    for e in &errors {
        assert!(!e.message.is_empty());
        assert!(e.range.end_byte as usize <= case.broken.len());
    }
    // Folding.
    assert!(
        stub.folding
            .iter()
            .any(|f| (f.start_line, f.end_line) == case.fold),
        "{lang:?}: no fold {:?} in {:?}",
        case.fold,
        stub.folding
    );
    assert!(stub.folding.iter().all(|f| f.end_line > f.start_line));
    // Highlight.
    let spans = highlight(lang, case.source).unwrap();
    for pair in spans.windows(2) {
        assert!(
            pair[0].end_byte <= pair[1].start_byte,
            "{lang:?}: spans overlap"
        );
    }
    for want in case.tokens {
        assert!(
            spans.iter().any(|s| s.kind == *want),
            "{lang:?}: no {want:?} token; kinds: {:?}",
            spans
                .iter()
                .map(|s| s.kind)
                .collect::<std::collections::BTreeSet<_>>()
                .iter()
                .map(|k| format!("{k:?}"))
                .collect::<Vec<_>>()
        );
    }
    // The stub round-trips through serde and a second parse gives the same stub.
    let json = serde_json::to_string(&stub).unwrap();
    let back: Stub = serde_json::from_str(&json).unwrap();
    assert_eq!(back, stub);
    assert_eq!(Stub::parse(lang, case.source).unwrap(), stub);
    // No reference is the name of a declaration at the same place.
    for r in &stub.references {
        assert!(
            !stub.declarations.iter().any(|d| d.name_range == r.range),
            "{lang:?}: reference {} is a declaration",
            r.name
        );
    }
    // The outline holds every declaration once.
    fn count(items: &[aivory_syntax::OutlineItem]) -> usize {
        items.iter().map(|i| 1 + count(&i.children)).sum()
    }
    assert_eq!(
        count(&stub.outline),
        stub.declarations.len(),
        "{lang:?} outline"
    );
}

#[test]
fn rust() {
    let case = Case {
        lang: L::Rust,
        source: r#"use std::collections::HashMap as Map;
use std::io::{self, Read};

/// A point.
pub struct Point {
    pub x: i32,
    y: i32,
}

pub enum Shape {
    Circle,
    Square(i32),
}

pub trait Area {
    fn area(&self) -> f64;
}

impl Area for Point {
    fn area(&self) -> f64 {
        0.0
    }
}

impl Point {
    /// Make one.
    #[inline]
    pub fn new(x: i32, y: i32) -> Self {
        Point { x, y }
    }
    fn hidden(&self) {}
}

pub(crate) const LIMIT: usize = 10;

mod inner {
    pub fn deep() {}
}

macro_rules! twice {
    ($e:expr) => {
        $e * 2
    };
}
"#,
        decls: &[
            ("Point", Struct, None),
            ("x", Field, Some("Point")),
            ("Shape", Enum, None),
            ("Circle", Variant, Some("Shape")),
            ("Area", Trait, None),
            ("area", Method, Some("Area")),
            ("Point", Impl, None),
            ("new", Method, Some("Point")),
            ("hidden", Method, Some("Point")),
            ("LIMIT", Constant, None),
            ("inner", Module, None),
            ("deep", Function, Some("inner")),
            ("twice", Macro, None),
        ],
        imports: &[
            ("std::collections::HashMap", Some("Map")),
            ("std::io::{self,Read}", None),
        ],
        broken: "fn main() { let x = 1 }\nfn g( {",
        fold: (4, 7),
        tokens: &[
            T::Keyword,
            T::Type,
            T::Function,
            T::Comment,
            T::Number,
            T::Punctuation,
        ],
    };
    run(&case);
    let stub = Stub::parse(L::Rust, case.source).unwrap();
    let new = find(&stub, "new", Method);
    assert_eq!(new.visibility, Public);
    assert!(new.doc.is_some(), "doc comment of new");
    assert!(
        new.signature
            .starts_with("pub fn new(x: i32, y: i32) -> Self")
    );
    assert_eq!(find(&stub, "hidden", Method).visibility, Private);
    assert_eq!(find(&stub, "LIMIT", Constant).visibility, Internal);
    assert!(find(&stub, "Point", Struct).doc.is_some());
    assert_eq!(find(&stub, "Circle", Variant).visibility, Public);
    // The trait impl method is public. The self type of the impl is the name.
    assert!(stub.declarations.iter().any(|d| d.name == "area"
        && d.container.as_deref() == Some("Point")
        && d.visibility == Public));
    // Message of a missing token.
    let errors = syntax_errors(L::Rust, "fn main() { let x = 1 }").unwrap();
    assert!(
        errors
            .iter()
            .any(|e| e.kind == SyntaxErrorKind::Missing && e.message == "missing `;`"),
        "{errors:?}"
    );
    // The reference `HashMap` is part of an import: it does not appear as a reference.
    assert!(!stub.references.iter().any(|r| r.name == "HashMap"));
    assert!(stub.references.iter().any(|r| r.name == "Point"));
}

#[test]
fn typescript() {
    let case = Case {
        lang: L::TypeScript,
        source: r#"import { readFile } from "fs";
import * as path from "path";
import React from "react";

/** A user. */
export interface User {
  id: number;
  name?: string;
}
export type Id = number | string;
export enum Color {
  Red,
  Green = 2,
}
export abstract class Base {
  abstract stop(): void;
}
export class Service extends Base {
  private cache = new Map<string, User>();
  #secret = 1;
  constructor(private readonly name: string) {
    super();
  }
  public run(): void {}
  protected helper(x: number): number {
    return x;
  }
}
export function load(id: Id): User {
  return { id: 1 };
}
export const answer = 42;
const local = (a: number) => a + 1;
namespace Util {
  export function noop() {}
}
"#,
        decls: &[
            ("User", Interface, None),
            ("id", Property, Some("User")),
            ("Id", TypeAlias, None),
            ("Color", Enum, None),
            ("Red", Variant, Some("Color")),
            ("Green", Variant, Some("Color")),
            ("Base", Class, None),
            ("Service", Class, None),
            ("cache", Field, Some("Service")),
            ("#secret", Field, Some("Service")),
            ("constructor", Constructor, Some("Service")),
            ("run", Method, Some("Service")),
            ("helper", Method, Some("Service")),
            ("load", Function, None),
            ("answer", Constant, None),
            ("local", Function, None),
            ("Util", Namespace, None),
            ("noop", Function, Some("Util")),
        ],
        imports: &[
            ("fs", None),
            ("path", Some("path")),
            ("react", Some("React")),
        ],
        broken: "function f( { return 1 }",
        fold: (5, 8),
        tokens: &[
            T::Keyword,
            T::Type,
            T::Function,
            T::String,
            T::Number,
            T::Punctuation,
        ],
    };
    run(&case);
    let stub = Stub::parse(L::TypeScript, case.source).unwrap();
    assert_eq!(find(&stub, "helper", Method).visibility, Protected);
    assert_eq!(find(&stub, "#secret", Field).visibility, Private);
    assert_eq!(find(&stub, "load", Function).visibility, Public);
    assert_eq!(find(&stub, "local", Function).visibility, Internal);
    assert!(find(&stub, "User", Interface).doc.is_some());
}

#[test]
fn tsx() {
    let case = Case {
        lang: L::Tsx,
        source: r#"import React from "react";

export function App(props: { n: number }) {
  return (
    <div className="x">
      {props.n}
    </div>
  );
}

export class Box extends React.Component {
  render() {
    return <span />;
  }
}
"#,
        decls: &[
            ("App", Function, None),
            ("Box", Class, None),
            ("render", Method, Some("Box")),
        ],
        imports: &[("react", Some("React"))],
        broken: "const a = <div>;\nfunction f( {",
        fold: (2, 8),
        tokens: &[T::Keyword, T::String],
    };
    run(&case);
}

#[test]
fn javascript() {
    let case = Case {
        lang: L::JavaScript,
        source: r#"const fs = require("fs");
import { a as b } from "./mod.js";

class Counter {
  #n = 0;
  static total = 0;
  constructor() {
    this.#n = 1;
  }
  inc() {
    return ++this.#n;
  }
}

function helper(x) {
  return x * 2;
}
const arrow = (x) => x + 1;
var legacy = 1;
let counter = 0;
export default Counter;
"#,
        decls: &[
            ("fs", Constant, None),
            ("Counter", Class, None),
            ("#n", Field, Some("Counter")),
            ("total", Field, Some("Counter")),
            ("constructor", Constructor, Some("Counter")),
            ("inc", Method, Some("Counter")),
            ("helper", Function, None),
            ("arrow", Function, None),
            ("legacy", Variable, None),
            ("counter", Variable, None),
        ],
        imports: &[("fs", None), ("./mod.js", None)],
        broken: "function f( { return 1 }",
        fold: (3, 12),
        tokens: &[
            T::Keyword,
            T::Function,
            T::String,
            T::Number,
            T::Punctuation,
            T::Operator,
        ],
    };
    run(&case);
}

#[test]
fn python() {
    let case = Case {
        lang: L::Python,
        source: r#"import os
import numpy as np
from collections import OrderedDict
from . import sibling

MAX_SIZE = 10


class Shape:
    """A shape."""

    sides = 0

    def __init__(self, sides):
        self.sides = sides

    def _private(self):
        pass

    @staticmethod
    def make():
        return Shape(3)


def area(shape):
    """Area of a shape."""
    return 0
"#,
        decls: &[
            ("MAX_SIZE", Constant, None),
            ("Shape", Class, None),
            ("sides", Field, Some("Shape")),
            ("__init__", Constructor, Some("Shape")),
            ("_private", Method, Some("Shape")),
            ("make", Method, Some("Shape")),
            ("area", Function, None),
        ],
        imports: &[
            ("os", None),
            ("numpy", Some("np")),
            ("collections", None),
            (".", None),
        ],
        broken: "def f(:\n    pass\n",
        fold: (8, 21),
        tokens: &[T::Keyword, T::Function, T::String, T::Number, T::Operator],
    };
    run(&case);
    let stub = Stub::parse(L::Python, case.source).unwrap();
    assert!(find(&stub, "Shape", Class).doc.is_some(), "docstring");
    assert!(find(&stub, "area", Function).doc.is_some(), "docstring");
    assert!(find(&stub, "make", Method).doc.is_none());
    assert_eq!(find(&stub, "_private", Method).visibility, Private);
    assert_eq!(find(&stub, "__init__", Constructor).visibility, Public);
}

#[test]
fn go() {
    let case = Case {
        lang: L::Go,
        source: r#"package main

import (
	"fmt"
	str "strings"
)

const Version = "1"

var counter int

// Server serves requests.
type Server struct {
	Name string
	port int
}

type Handler interface {
	Handle(req string) error
}

type ID int

func (s *Server) Run() error {
	return nil
}

func helper() {
	fmt.Println(str.ToUpper("x"))
}
"#,
        decls: &[
            ("main", Module, None),
            ("Version", Constant, None),
            ("counter", Variable, None),
            ("Server", Struct, None),
            ("Name", Field, Some("Server")),
            ("port", Field, Some("Server")),
            ("Handler", Interface, None),
            ("Handle", Method, Some("Handler")),
            ("ID", TypeAlias, None),
            ("Run", Method, Some("Server")),
            ("helper", Function, None),
        ],
        imports: &[("fmt", None), ("strings", Some("str"))],
        broken: "func f( {\n",
        fold: (12, 15),
        tokens: &[T::Keyword, T::Function, T::String, T::Type],
    };
    run(&case);
    let stub = Stub::parse(L::Go, case.source).unwrap();
    assert!(find(&stub, "Server", Struct).doc.is_some());
    assert_eq!(find(&stub, "Run", Method).visibility, Public);
    assert_eq!(find(&stub, "helper", Function).visibility, Private);
    // The method nests below its receiver type in the outline.
    let server = stub.outline.iter().find(|o| o.name == "Server").unwrap();
    assert!(server.children.iter().any(|c| c.name == "Run"));
}

#[test]
fn java() {
    let case = Case {
        lang: L::Java,
        source: r#"package com.example;

import java.util.List;
import java.util.Map;

/** A service. */
public class Service implements Runnable {
    private int count;
    public static final int MAX = 3;

    public Service() {}

    public void run() {
    }

    protected static String name() {
        return "";
    }
}

interface Greeter {
    String greet(String n);
}

enum Mode { ON, OFF }
"#,
        decls: &[
            ("com.example", Module, None),
            ("Service", Class, None),
            ("count", Field, Some("Service")),
            ("MAX", Field, Some("Service")),
            ("Service", Constructor, Some("Service")),
            ("run", Method, Some("Service")),
            ("name", Method, Some("Service")),
            ("Greeter", Interface, None),
            ("greet", Method, Some("Greeter")),
            ("Mode", Enum, None),
            ("ON", Variant, Some("Mode")),
        ],
        imports: &[("java.util.List", None), ("java.util.Map", None)],
        broken: "class A { void f( { } }",
        fold: (6, 18),
        tokens: &[T::Keyword, T::Type, T::String, T::Comment],
    };
    run(&case);
    let stub = Stub::parse(L::Java, case.source).unwrap();
    assert_eq!(find(&stub, "Service", Class).visibility, Public);
    assert_eq!(find(&stub, "count", Field).visibility, Private);
    assert_eq!(find(&stub, "name", Method).visibility, Protected);
    assert_eq!(find(&stub, "Greeter", Interface).visibility, Internal);
    assert_eq!(find(&stub, "greet", Method).visibility, Public);
    assert!(find(&stub, "Service", Class).doc.is_some());
    // Two imports in a row fold as one region.
    assert!(stub.folding.iter().any(
        |f| f.kind == aivory_syntax::FoldKind::Imports && (f.start_line, f.end_line) == (2, 3)
    ));
}

#[test]
fn c() {
    let case = Case {
        lang: L::C,
        source: r#"#include <stdio.h>
#include "local.h"
#define MAX 10
#define SQUARE(x) ((x) * (x))

struct Point {
    int x;
    int y;
};
typedef struct Point Pt;
enum Color { RED, GREEN };
static int counter = 0;

int add(int a, int b) {
    return a + b;
}

static void helper(void) {}
int proto(int);
"#,
        decls: &[
            ("MAX", Macro, None),
            ("SQUARE", Macro, None),
            ("Point", Struct, None),
            ("x", Field, Some("Point")),
            ("Pt", TypeAlias, None),
            ("Color", Enum, None),
            ("RED", Variant, Some("Color")),
            ("counter", Variable, None),
            ("add", Function, None),
            ("helper", Function, None),
            ("proto", Function, None),
        ],
        imports: &[("stdio.h", None), ("local.h", None)],
        broken: "int f( {\n",
        fold: (5, 8),
        tokens: &[T::Keyword, T::Type, T::Function, T::String, T::Number],
    };
    run(&case);
    let stub = Stub::parse(L::C, case.source).unwrap();
    assert_eq!(find(&stub, "counter", Variable).visibility, Private);
    assert_eq!(find(&stub, "helper", Function).visibility, Private);
    assert_eq!(find(&stub, "add", Function).visibility, Public);
}

#[test]
fn cpp() {
    let case = Case {
        lang: L::Cpp,
        source: r#"#include <vector>

namespace geo {

class Shape {
public:
    Shape() {}
    virtual double area() const = 0;

private:
    int id;
};

struct Pt {
    int x;
};

double Shape::area() const {
    return 0;
}

}

template <typename T>
T twice(T v) {
    return v + v;
}

using Id = int;
"#,
        decls: &[
            ("geo", Namespace, None),
            ("Shape", Class, Some("geo")),
            ("Shape", Constructor, Some("Shape")),
            ("area", Method, Some("Shape")),
            ("id", Field, Some("Shape")),
            ("Pt", Struct, Some("geo")),
            ("twice", Function, None),
            ("Id", TypeAlias, None),
        ],
        imports: &[("vector", None)],
        broken: "int f( {\n",
        fold: (4, 11),
        tokens: &[T::Keyword, T::Type, T::Function, T::Number],
    };
    run(&case);
    let stub = Stub::parse(L::Cpp, case.source).unwrap();
    assert_eq!(find(&stub, "id", Field).visibility, Private);
    assert_eq!(find(&stub, "Pt", Struct).visibility, Public);
    // The out-of-class definition belongs to its class.
    assert!(
        stub.declarations
            .iter()
            .filter(|d| d.name == "area" && d.container.as_deref() == Some("Shape"))
            .count()
            == 2
    );
}

#[test]
fn csharp() {
    let case = Case {
        lang: L::CSharp,
        source: r#"using System;
using System.Collections.Generic;
using Alias = System.Text;

namespace Demo
{
    /// <summary>A person.</summary>
    public class Person
    {
        private int age;
        public string Name { get; set; }
        public Person() {}
        public void Greet() {}
        protected internal void Both() {}
    }

    internal interface IThing { void Do(); }

    public enum Kind { A, B }

    public struct Pt { public int X; }
}
"#,
        decls: &[
            ("Demo", Namespace, None),
            ("Person", Class, Some("Demo")),
            ("age", Field, Some("Person")),
            ("Name", Property, Some("Person")),
            ("Person", Constructor, Some("Person")),
            ("Greet", Method, Some("Person")),
            ("Both", Method, Some("Person")),
            ("IThing", Interface, Some("Demo")),
            ("Do", Method, Some("IThing")),
            ("Kind", Enum, Some("Demo")),
            ("A", Variant, Some("Kind")),
            ("Pt", Struct, Some("Demo")),
            ("X", Field, Some("Pt")),
        ],
        imports: &[
            ("System", None),
            ("System.Collections.Generic", None),
            ("System.Text", Some("Alias")),
        ],
        broken: "class A { void F( { } }",
        fold: (5, 21),
        tokens: &[T::Keyword, T::Type, T::Comment],
    };
    run(&case);
    let stub = Stub::parse(L::CSharp, case.source).unwrap();
    assert_eq!(find(&stub, "Person", Class).visibility, Public);
    assert_eq!(find(&stub, "age", Field).visibility, Private);
    assert_eq!(find(&stub, "Both", Method).visibility, Protected);
    assert_eq!(find(&stub, "IThing", Interface).visibility, Internal);
    assert_eq!(find(&stub, "Do", Method).visibility, Public);
    assert!(find(&stub, "Person", Class).doc.is_some());
}

#[test]
fn kotlin() {
    let case = Case {
        lang: L::Kotlin,
        source: r#"package demo

import kotlin.math.max
import java.util.List as JList

/** A thing. */
class Thing(val id: Int) {
    private val secret = 1
    fun run() {}
}

interface Named {
    fun name(): String
}

object Registry {
    val items = 1
}

enum class Level { LOW, HIGH }

fun top(x: Int): Int = x
val CONST = 5
typealias Id = Int
"#,
        decls: &[
            ("demo", Module, None),
            ("Thing", Class, None),
            ("secret", Property, Some("Thing")),
            ("run", Method, Some("Thing")),
            ("Named", Interface, None),
            ("name", Method, Some("Named")),
            ("Registry", Class, None),
            ("items", Property, Some("Registry")),
            ("Level", Enum, None),
            ("LOW", Variant, Some("Level")),
            ("top", Function, None),
            ("CONST", Property, None),
            ("Id", TypeAlias, None),
        ],
        imports: &[("kotlin.math.max", None), ("java.util.List", Some("JList"))],
        broken: "fun f( {\n",
        fold: (6, 9),
        tokens: &[T::Keyword, T::Comment, T::Number],
    };
    run(&case);
    let stub = Stub::parse(L::Kotlin, case.source).unwrap();
    assert_eq!(find(&stub, "secret", Property).visibility, Private);
    assert_eq!(find(&stub, "run", Method).visibility, Public);
    assert!(find(&stub, "Thing", Class).doc.is_some());
}

#[test]
fn ruby() {
    let case = Case {
        lang: L::Ruby,
        source: r#"require "json"
require_relative "helper"

module Util
  VERSION = "1"

  def self.build
  end
end

# A dog.
class Dog < Animal
  MAX = 3

  def initialize(name)
    @name = name
  end

  def bark
    puts "woof"
  end
end
"#,
        decls: &[
            ("Util", Module, None),
            ("VERSION", Constant, Some("Util")),
            ("build", Function, Some("Util")),
            ("Dog", Class, None),
            ("MAX", Constant, Some("Dog")),
            ("initialize", Constructor, Some("Dog")),
            ("bark", Method, Some("Dog")),
        ],
        imports: &[("json", None), ("helper", None)],
        broken: "def f(\n",
        fold: (11, 21),
        tokens: &[T::Keyword, T::String, T::Comment],
    };
    run(&case);
    let stub = Stub::parse(L::Ruby, case.source).unwrap();
    assert!(find(&stub, "Dog", Class).doc.is_some());
}

#[test]
fn php() {
    let case = Case {
        lang: L::Php,
        source: r#"<?php
namespace App\Models;

use App\Base\Model;
use Lib\Helper as H;

require_once "vendor/autoload.php";

/** A user. */
class User extends Model {
    private $name;
    const ROLE = 1;
    public function __construct() {}
    protected function save() {
    }
}

interface Named {
    public function name();
}

trait Loggable {
}

enum Suit {
    case Hearts;
}

function helper() {
}
"#,
        decls: &[
            ("App\\Models", Namespace, None),
            ("User", Class, None),
            ("$name", Field, Some("User")),
            ("ROLE", Constant, Some("User")),
            ("__construct", Constructor, Some("User")),
            ("save", Method, Some("User")),
            ("Named", Interface, None),
            ("name", Method, Some("Named")),
            ("Loggable", Trait, None),
            ("Suit", Enum, None),
            ("Hearts", Variant, Some("Suit")),
            ("helper", Function, None),
        ],
        imports: &[
            ("App\\Base\\Model", None),
            ("Lib\\Helper", Some("H")),
            ("vendor/autoload.php", None),
        ],
        broken: "<?php function f( { }",
        fold: (9, 15),
        tokens: &[T::Keyword, T::String, T::Comment],
    };
    run(&case);
    let stub = Stub::parse(L::Php, case.source).unwrap();
    assert_eq!(find(&stub, "$name", Field).visibility, Private);
    assert_eq!(find(&stub, "save", Method).visibility, Protected);
    assert!(find(&stub, "User", Class).doc.is_some());
}

#[test]
fn bash() {
    let case = Case {
        lang: L::Bash,
        source: r#"#!/bin/bash
source ./lib.sh
. ./other.sh
NAME="x"
greet() {
  echo "hi $1"
}
function bye {
  echo bye
}
"#,
        decls: &[
            ("NAME", Variable, None),
            ("greet", Function, None),
            ("bye", Function, None),
        ],
        imports: &[("./lib.sh", None), ("./other.sh", None)],
        broken: "if [ 1 ]; then\n  echo hi\n",
        fold: (4, 6),
        tokens: &[T::Keyword, T::String, T::Comment],
    };
    run(&case);
}

#[test]
fn json() {
    let case = Case {
        lang: L::Json,
        source: r#"{
  "name": "x",
  "nested": {
    "a": 1,
    "deep": {
      "z": 1
    }
  },
  "list": [
    1,
    2
  ]
}
"#,
        decls: &[
            ("name", Property, None),
            ("nested", Property, None),
            ("a", Property, Some("nested")),
            ("deep", Property, Some("nested")),
            ("list", Property, None),
        ],
        imports: &[],
        broken: "{\"a\": }",
        fold: (2, 7),
        tokens: &[T::String, T::Number],
    };
    run(&case);
    let stub = Stub::parse(L::Json, case.source).unwrap();
    // Only two levels of keys: `z` is too deep.
    assert!(!stub.declarations.iter().any(|d| d.name == "z"));
}

#[test]
fn toml() {
    let case = Case {
        lang: L::Toml,
        source: r#"title = "x"

[package]
name = "demo"
version = "1"

[[bin]]
path = "a"
"#,
        decls: &[
            ("title", Property, None),
            ("package", Section, None),
            ("name", Property, Some("package")),
            ("version", Property, Some("package")),
            ("bin", Section, None),
            ("path", Property, Some("bin")),
        ],
        imports: &[],
        broken: "a = \n[b\n",
        fold: (2, 4),
        tokens: &[T::String],
    };
    run(&case);
}

#[test]
fn yaml() {
    let case = Case {
        lang: L::Yaml,
        source: r#"name: demo
services:
  web:
    image: nginx
    ports:
      - 80
list:
  - a
  - b
"#,
        decls: &[
            ("name", Property, None),
            ("services", Property, None),
            ("web", Property, Some("services")),
            ("list", Property, None),
        ],
        imports: &[],
        broken: "a: [1, 2\nb: 3\n",
        fold: (1, 5),
        tokens: &[T::String],
    };
    run(&case);
    let stub = Stub::parse(L::Yaml, case.source).unwrap();
    assert!(!stub.declarations.iter().any(|d| d.name == "image"));
}

#[test]
fn html() {
    let case = Case {
        lang: L::Html,
        source: r#"<!DOCTYPE html>
<html>
  <body>
    <div id="main">
      <p>hi</p>
    </div>
  </body>
</html>
"#,
        decls: &[("main", Section, None)],
        imports: &[],
        broken: "<div class=\"a\n<p>",
        fold: (3, 5),
        tokens: &[T::Keyword],
    };
    run(&case);
}

#[test]
fn css() {
    let case = Case {
        lang: L::Css,
        source: r#"@import "base.css";

:root {
  --main: red;
}

.btn {
  color: red;
}

@keyframes spin {
  from { opacity: 0; }
  to { opacity: 1; }
}
"#,
        decls: &[
            (":root", Section, None),
            ("--main", Variable, Some(":root")),
            (".btn", Section, None),
            ("spin", Section, None),
        ],
        imports: &[("base.css", None)],
        broken: ".a { color: red;\n.b {",
        fold: (6, 8),
        tokens: &[T::Number, T::Keyword],
    };
    run(&case);
}

#[test]
fn detect_language() {
    let cases = [
        ("src/main.rs", L::Rust),
        ("a/b/app.ts", L::TypeScript),
        ("App.tsx", L::Tsx),
        ("x.mjs", L::JavaScript),
        ("x.jsx", L::JavaScript),
        ("tool.PY", L::Python),
        ("main.go", L::Go),
        ("A.java", L::Java),
        ("a.c", L::C),
        ("a.h", L::C),
        ("a.cpp", L::Cpp),
        ("a.hpp", L::Cpp),
        ("A.cs", L::CSharp),
        ("a.json", L::Json),
        ("run.sh", L::Bash),
        (".bashrc", L::Bash),
        ("A.kt", L::Kotlin),
        ("a.rb", L::Ruby),
        ("Rakefile", L::Ruby),
        ("Gemfile", L::Ruby),
        ("a.php", L::Php),
        ("Cargo.toml", L::Toml),
        ("Cargo.lock", L::Toml),
        ("ci.yml", L::Yaml),
        ("a.yaml", L::Yaml),
        ("index.html", L::Html),
        ("site.css", L::Css),
    ];
    for (path, want) in cases {
        assert_eq!(Language::detect(path), Some(want), "{path}");
    }
    assert_eq!(Language::detect("README"), None);
    assert_eq!(Language::detect("a.unknown"), None);
    assert_eq!(Language::ALL.len(), 19);
    for lang in Language::ALL {
        // A grammar version comes from Cargo.lock.
        assert!(
            !lang.grammar_version().contains("unknown"),
            "{lang:?}: {}",
            lang.grammar_version()
        );
        // The name is the serialized form.
        assert_eq!(
            serde_json::to_string(&lang).unwrap(),
            format!("\"{}\"", lang.name())
        );
        // Every language parses an empty text.
        let stub = Stub::parse(lang, "").unwrap();
        assert!(stub.declarations.is_empty());
    }
}

#[test]
fn stub_key_and_hash() {
    let a = Stub::parse(L::Rust, "fn a() {}").unwrap();
    let b = Stub::parse(L::Rust, "fn b() {}").unwrap();
    assert_eq!(a.hash.len(), 64);
    assert_ne!(a.hash, b.hash);
    assert_eq!(a.hash, blake3_hex("fn a() {}"));
    assert!(a.key().starts_with(&a.hash));
    assert!(a.grammar_version.starts_with("tree-sitter-rust "));
    assert_eq!(a.extractor_version, aivory_syntax::EXTRACTOR_VERSION);
}

fn blake3_hex(s: &str) -> String {
    Document::new(L::Rust, s).unwrap().hash()
}
