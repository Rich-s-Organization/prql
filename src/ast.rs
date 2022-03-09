use super::utils::*;
use crate::ast_fold::AstFold;
use anyhow::{anyhow, Result};
use itertools::Itertools;
use serde::{Deserialize, Serialize};

// Idents are generally columns
pub type Ident = String;
pub type Items = Vec<Item>;
pub type Idents = Vec<Ident>;
pub type Pipeline = Vec<Transformation>;

use enum_as_inner::EnumAsInner;

#[derive(Debug, EnumAsInner, PartialEq, Clone, Serialize, Deserialize)]
pub enum Item {
    Transformation(Transformation),
    Ident(Ident),
    String(String),
    Raw(String),
    Assign(Assign),
    NamedArg(NamedArg),
    // TODO: Add dialect & prql version onto Query.
    Query(Items),
    Pipeline(Pipeline),
    // Similar to holding an Expr, but we strongly type it so the parsing can be more strict.
    List(Vec<ListItem>),
    // Holds "Terms", not including separators like `+`. Unnesting this (i.e.
    // Terms([Item]) -> Item) does not change its semantics. (More detail in
    // `prql.pest`)
    Terms(Items),
    // Holds any Items. Unnesting _can_ change semantics (though it's less
    // important than when this was used as a ListItem).
    Items(Items),
    Idents(Idents),
    Function(Function),
    Table(Table),
    SString(Vec<SStringItem>),
    // Anything not yet implemented.
    Todo(String),
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct ListItem(pub Items);

impl ListItem {
    pub fn into_inner(self) -> Items {
        self.0
    }
}

/// Transformation is currently used for a) each transformation in a pipeline
/// and sometimes b) a normal function call. But we want to resolve whether (b)
/// should apply or not.
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
// We probably want to implement some of these as Structs rather than just
// `Items`
pub enum Transformation {
    From(Ident),
    Select(Items),
    Filter(Filter),
    Derive(Vec<Assign>),
    Aggregate {
        by: Vec<Item>,
        calcs: Vec<Item>,
        assigns: Vec<Assign>,
    },
    Sort(Items),
    Take(i64),
    Join(Items),
    Func(FuncCall),
}

impl Transformation {
    /// Returns the name of the transformation.
    pub fn name(&self) -> &str {
        match self {
            Transformation::From(_) => "from",
            Transformation::Select(_) => "select",
            Transformation::Filter(_) => "filter",
            Transformation::Derive(_) => "derive",
            Transformation::Aggregate { .. } => "aggregate",
            Transformation::Sort(_) => "sort",
            Transformation::Take(_) => "take",
            Transformation::Join(_) => "join",
            // Currently this is unused, since we don't encode function calls as
            // anything more than Idents at the moment. We may want to change
            // that in the future.
            Transformation::Func(FuncCall { name, .. }) => name,
        }
    }
}

/// Function definition.
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct Function {
    pub name: Ident,
    pub args: Vec<Ident>,
    pub body: Items,
}

/// Function call.
#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct FuncCall {
    pub name: String,
    pub args: Items,
    pub named_args: Vec<NamedArg>,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct Table {
    pub name: Ident,
    pub pipeline: Pipeline,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct NamedArg {
    pub name: Ident,
    pub arg: Box<Item>,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct Assign {
    pub lvalue: Ident,
    pub rvalue: Box<Item>,
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub enum SStringItem {
    String(String),
    Expr(Item),
}

#[derive(Debug, PartialEq, Clone, Serialize, Deserialize)]
pub struct Filter(pub Items);

// We've done a lot of iteration on these containers, and it's still very messy.
// Some of the tradeoff is having an Enum which is flexible, but not falling
// back to dynamic types, which makes understanding what the parser is doing
// more difficult.
impl Item {
    /// Either provide a Vec with the contents of Items / Terms / Query, or puts a scalar
    /// into a Vec. This is useful when we either have a scalar or a list, and
    /// want to only have to handle a single type.
    pub fn into_inner_items(self) -> Vec<Item> {
        match self {
            Item::Terms(items) | Item::Items(items) | Item::Query(items) => items,
            _ => vec![self],
        }
    }
    pub fn as_inner_items(&self) -> Result<&Vec<Item>> {
        if let Item::Terms(items) | Item::Items(items) | Item::Query(items) = self {
            Ok(items)
        } else {
            Err(anyhow!("Expected container type; got {self:?}"))
        }
    }
    pub fn into_inner_list_items(self) -> Result<Vec<Vec<Item>>> {
        match self {
            Item::List(items) => Ok(items.into_iter().map(|item| item.into_inner()).collect()),
            _ => Err(anyhow!("Expected a list, got {self:?}")),
        }
    }
    /// For lists that only have one item in each ListItem this returns a Vec of
    /// those terms. (e.g. `[1, a b]` but not `[1 + 2]`, because `+` in an
    /// operator and so will create an `Items` for each of `1` & `2`)
    pub fn into_inner_list_single_items(self) -> Result<Vec<Item>> {
        match self {
            Item::List(items) => items
                .into_iter()
                .map(|list_item| list_item.into_inner().into_only())
                .try_collect(),
            _ => Err(anyhow!("Expected a list, got {self:?}")),
        }
    }

    /// Wrap in Terms unless it's already a Terms.
    // TODO: not sure whether we really need this — it's not orthogonal to
    // `as_scalar` and `into_inner_items`. Ideally we can reduce the number of
    // these functions.
    pub fn coerce_to_terms(self) -> Item {
        match self {
            Item::Terms(_) => self,
            _ => Item::Terms(vec![self]),
        }
    }
    /// Either provide a List with the contents of `self`, or `self` if the item
    /// is already a list. This is useful when we either have a scalar or a
    /// list, and want to only have to handle a single type.
    pub fn coerce_to_list(self) -> Item {
        match self {
            Item::List(_) => self,
            _ => Item::List(vec![ListItem(vec![self])]),
        }
    }
    /// Make a list from a vec of Items
    pub fn into_list_of_items(items: Items) -> Item {
        Item::List(items.into_iter().map(|item| ListItem(vec![item])).collect())
    }

    /// The scalar version / opposite of `as_inner_items`. It keeps unwrapping
    /// Item / Expr types until it finds one with a non-single element.
    // TODO: I can't seem to get a move version of this that works with the
    // `.unwrap_or` at the end — is there a way?
    pub fn as_scalar(&self) -> &Item {
        match self {
            Item::Terms(items) | Item::Items(items) => {
                items.only().map(|item| item.as_scalar()).unwrap_or(self)
            }
            _ => self,
        }
    }
}

pub trait IntoUnnested {
    fn into_unnested(self) -> Self;
}
impl IntoUnnested for Item {
    /// Transitively unnest the whole tree, traversing even parents with more
    /// than one child. This is more unnesting that `as_scalar' does. Only
    /// removes `Terms` (not `Items` or `List`), though it does walk all the
    /// containers.
    fn into_unnested(self) -> Self {
        Unnest.fold_item(&self).unwrap()
    }
}

use super::ast_fold::fold_item;
struct Unnest;
impl AstFold for Unnest {
    fn fold_item(&mut self, item: &Item) -> Result<Item> {
        match item {
            Item::Terms(_) => fold_item(self, &item.as_scalar().clone()),
            _ => fold_item(self, item),
        }
    }
}

use anyhow::Error;
impl From<Item> for Error {
    // https://github.com/bluejekyll/enum-as-inner/issues/84
    fn from(item: Item) -> Self {
        anyhow!("Failed to convert {item:?}")
    }
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn test_as_scalar() {
        let atom = Item::Ident("a".to_string());

        // Gets the single item through one level of nesting.
        let item = Item::Terms(vec![atom.clone()]);
        assert_eq!(item.as_scalar(), &atom);

        // No change when it's the same.
        let item = atom.clone();
        assert_eq!(item.as_scalar(), &item);

        // No change when there are two items in the `terms`.
        let item = Item::Terms(vec![atom.clone(), atom.clone()]);
        assert_eq!(item.as_scalar(), &item);

        // Gets the single item through two levels of nesting.
        let item = Item::Terms(vec![Item::Terms(vec![atom.clone()])]);
        assert_eq!(item.as_scalar(), &atom);
    }

    #[test]
    fn test_into_unnested() {
        let atom = Item::Ident("a".to_string());

        // Gets the single item through one level of nesting.
        let item = Item::Terms(vec![atom.clone()]);
        assert_eq!(item.into_unnested(), atom);

        // No change when it's the same.
        let item = atom.clone();
        assert_eq!(item.clone().into_unnested(), item);

        // No change when there are two items in the `terms`.
        let item = Item::Terms(vec![atom.clone(), atom.clone()]);
        assert_eq!(item.clone().into_unnested(), item);

        // Gets the single item through two levels of nesting.
        let item = Item::Terms(vec![Item::Terms(vec![atom.clone()])]);
        assert_eq!(item.into_unnested(), atom);

        // Gets a single item through a parent which isn't nested
        let item = Item::Terms(vec![
            Item::Terms(vec![atom.clone()]),
            Item::Terms(vec![atom.clone()]),
        ]);
        assert_eq!(item.into_unnested(), Item::Terms(vec![atom.clone(), atom]));
    }
}
