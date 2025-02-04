// Copyright 2019 The xi-editor Authors.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Support for lenses, a way of focusing on subfields of data.

use std::marker::PhantomData;
use std::ops;
use std::sync::Arc;

pub use druid_derive::Lens;

use crate::kurbo::Size;
use crate::{
    BaseState, BoxConstraints, Data, Env, Event, EventCtx, LayoutCtx, PaintCtx, UpdateCtx, Widget,
};

/// A lens is a datatype that gives access to a part of a larger
/// data structure.
///
/// A simple example of a lens is a field of a struct; in this case,
/// the lens itself is zero-sized. Another case is accessing an array
/// element, in which case the lens contains the array index.
///
/// Many `Lens` implementations will be derived by macro, but custom
/// implementations are practical as well.
///
/// The name "lens" is inspired by the [Haskell lens] package, which
/// has generally similar goals. It's likely we'll develop more
/// sophistication, for example combinators to combine lenses.
///
/// [Haskell lens]: http://hackage.haskell.org/package/lens
pub trait Lens<T: ?Sized, U: ?Sized> {
    /// Get non-mut access to the field.
    ///
    /// Runs the supplied closure with a reference to the data. It's
    /// structured this way, as opposed to simply returning a reference,
    /// so that the data might be synthesized on-the-fly by the lens.
    fn with<V, F: FnOnce(&U) -> V>(&self, data: &T, f: F) -> V;

    /// Get mutable access to the field.
    ///
    /// This method is defined in terms of a closure, rather than simply
    /// yielding a mutable reference, because it is intended to be used
    /// with value-type data (also known as immutable data structures).
    /// For example, a lens for an immutable list might be implemented by
    /// cloning the list, giving the closure mutable access to the clone,
    /// then updating the reference after the closure returns.
    fn with_mut<V, F: FnOnce(&mut U) -> V>(&self, data: &mut T, f: F) -> V;
}

/// Helpers for manipulating `Lens`es
pub trait LensExt<A: ?Sized, B: ?Sized>: Lens<A, B> {
    /// Copy the targeted value out of `data`
    fn get(&self, data: &A) -> B
    where
        B: Clone,
    {
        self.with(data, |x| x.clone())
    }

    /// Set the targeted value in `data` to `value`
    fn put(&self, data: &mut A, value: B)
    where
        B: Sized,
    {
        self.with_mut(data, |x| *x = value);
    }

    /// Compose a `Lens<A, B>` with a `Lens<B, C>` to produce a `Lens<A, C>`
    ///
    /// ```
    /// # use druid::*;
    /// struct Foo { x: (u32, bool) }
    /// let lens = lens!(Foo, x).then(lens!((u32, bool), 1));
    /// assert_eq!(lens.get(&Foo { x: (0, true) }), true);
    /// ```
    fn then<Other, C>(self, other: Other) -> Then<Self, Other, B>
    where
        Other: Lens<B, C> + Sized,
        C: ?Sized,
        Self: Sized,
    {
        Then::new(self, other)
    }

    /// Combine a `Lens<A, B>` with a function that can transform a `B` and its inverse.
    ///
    /// Useful for cases where the desired value doesn't physically exist in `A`, but can be
    /// computed. For example, a lens like the following might be used to adapt a value with the
    /// range 0-2 for use with a `Widget<f64>` like `Slider` that has a range of 0-1:
    ///
    /// ```
    /// # use druid::*;
    /// let lens = lens!((bool, f64), 1);
    /// assert_eq!(lens.map(|x| x / 2.0, |x, y| *x = y * 2.0).get(&(true, 2.0)), 1.0);
    /// ```
    ///
    /// The computed `C` may represent a whole or only part of the original `B`.
    fn map<Get, Put, C>(self, get: Get, put: Put) -> Then<Self, Map<Get, Put>, B>
    where
        Get: Fn(&B) -> C,
        Put: Fn(&mut B, C),
        Self: Sized,
    {
        self.then(Map::new(get, put))
    }

    /// Invoke a type's `Deref` impl
    ///
    /// ```
    /// # use druid::*;
    /// assert_eq!(lens::Id.deref().get(&Box::new(42)), 42);
    /// ```
    fn deref(self) -> Then<Self, Deref, B>
    where
        B: ops::Deref + ops::DerefMut,
        Self: Sized,
    {
        self.then(Deref)
    }

    /// Access an index in a container
    ///
    /// ```
    /// # use druid::*;
    /// assert_eq!(lens::Id.index(2).get(&vec![0u32, 1, 2, 3]), 2);
    /// ```
    fn index<I>(self, index: I) -> Then<Self, Index<I>, B>
    where
        I: Clone,
        B: ops::Index<I> + ops::IndexMut<I>,
        Self: Sized,
    {
        self.then(Index::new(index))
    }

    /// Adapt to operate on the contents of an `Arc` with efficient copy-on-write semantics
    ///
    /// ```
    /// # use druid::*; use std::sync::Arc;
    /// let lens = lens::Id.index(2).in_arc();
    /// let mut x = Arc::new(vec![0, 1, 2, 3]);
    /// let original = x.clone();
    /// assert_eq!(lens.get(&x), 2);
    /// lens.put(&mut x, 2);
    /// assert!(Arc::ptr_eq(&original, &x), "no-op writes don't cause a deep copy");
    /// lens.put(&mut x, 42);
    /// assert_eq!(&*x, &[0, 1, 42, 3]);
    /// ```
    fn in_arc(self) -> InArc<Self>
    where
        A: Clone,
        B: Data,
        Self: Sized,
    {
        InArc::new(self)
    }
}

impl<A: ?Sized, B: ?Sized, T: Lens<A, B>> LensExt<A, B> for T {}

// A case can be made this should be in the `widget` module.

/// A wrapper for its widget subtree to have access to a part
/// of its parent's data.
///
/// Every widget in druid is instantiated with access to data of some
/// type; the root widget has access to the entire application data.
/// Often, a part of the widget hierarchy is only concerned with a part
/// of that data. The `LensWrap` widget is a way to "focus" the data
/// reference down, for the subtree. One advantage is performance;
/// data changes that don't intersect the scope of the lens aren't
/// propagated.
///
/// Another advantage is generality and reuse. If a widget (or tree of
/// widgets) is designed to work with some chunk of data, then with a
/// lens that same code can easily be reused across all occurrences of
/// that chunk within the application state.
///
/// This wrapper takes a [`Lens`] as an argument, which is a specification
/// of a struct field, or some other way of narrowing the scope.
///
/// [`Lens`]: trait.Lens.html
pub struct LensWrap<U, L, W> {
    inner: W,
    lens: L,
    // The following is a workaround for otherwise getting E0207.
    phantom: PhantomData<U>,
}

impl<U, L, W> LensWrap<U, L, W> {
    /// Wrap a widget with a lens.
    ///
    /// When the lens has type `Lens<T, U>`, the inner widget has data
    /// of type `U`, and the wrapped widget has data of type `T`.
    pub fn new(inner: W, lens: L) -> LensWrap<U, L, W> {
        LensWrap {
            inner,
            lens,
            phantom: Default::default(),
        }
    }
}

impl<T, U, L, W> Widget<T> for LensWrap<U, L, W>
where
    T: Data,
    U: Data,
    L: Lens<T, U>,
    W: Widget<U>,
{
    fn event(&mut self, ctx: &mut EventCtx, event: &Event, data: &mut T, env: &Env) {
        let inner = &mut self.inner;
        self.lens
            .with_mut(data, |data| inner.event(ctx, event, data, env))
    }

    fn update(&mut self, ctx: &mut UpdateCtx, old_data: Option<&T>, data: &T, env: &Env) {
        let inner = &mut self.inner;
        let lens = &self.lens;
        if let Some(old_data) = old_data {
            lens.with(old_data, |old_data| {
                lens.with(data, |data| {
                    if !old_data.same(data) {
                        inner.update(ctx, Some(old_data), data, env);
                    }
                })
            })
        } else {
            lens.with(data, |data| inner.update(ctx, None, data, env));
        }
    }

    fn layout(&mut self, ctx: &mut LayoutCtx, bc: &BoxConstraints, data: &T, env: &Env) -> Size {
        let inner = &mut self.inner;
        self.lens
            .with(data, |data| inner.layout(ctx, bc, data, env))
    }

    fn paint(&mut self, paint_ctx: &mut PaintCtx, base_state: &BaseState, data: &T, env: &Env) {
        let inner = &mut self.inner;
        self.lens
            .with(data, |data| inner.paint(paint_ctx, base_state, data, env));
    }
}

/// Lens accessing a member of some type using accessor functions
///
/// See also the `lens` macro.
///
/// ```
/// let lens = druid::lens::Field::new(|x: &Vec<u32>| &x[42], |x| &mut x[42]);
/// ```
pub struct Field<Get, GetMut> {
    get: Get,
    get_mut: GetMut,
}

impl<Get, GetMut> Field<Get, GetMut> {
    /// Construct a lens from a pair of getter functions
    pub fn new<T: ?Sized, U: ?Sized>(get: Get, get_mut: GetMut) -> Self
    where
        Get: Fn(&T) -> &U,
        GetMut: Fn(&mut T) -> &mut U,
    {
        Self { get, get_mut }
    }
}

impl<T, U, Get, GetMut> Lens<T, U> for Field<Get, GetMut>
where
    T: ?Sized,
    U: ?Sized,
    Get: Fn(&T) -> &U,
    GetMut: Fn(&mut T) -> &mut U,
{
    fn with<V, F: FnOnce(&U) -> V>(&self, data: &T, f: F) -> V {
        f((self.get)(data))
    }

    fn with_mut<V, F: FnOnce(&mut U) -> V>(&self, data: &mut T, f: F) -> V {
        f((self.get_mut)(data))
    }
}

/// Construct a lens accessing a type's field
///
/// This is a convenience macro for constructing `Field` lenses for fields or indexable elements.
///
/// ```
/// struct Foo { x: u32 }
/// let lens = druid::lens!(Foo, x);
/// let lens = druid::lens!((u32, bool), 1);
/// let lens = druid::lens!([u8], [4]);
/// ```
#[macro_export]
macro_rules! lens {
    ($ty:ty, [$index:expr]) => {
        $crate::lens::Field::new::<$ty, _>(|x| &x[$index], |x| &mut x[$index])
    };
    ($ty:ty, $field:tt) => {
        $crate::lens::Field::new::<$ty, _>(|x| &x.$field, |x| &mut x.$field)
    };
}

/// `Lens` composed of two lenses joined together
#[derive(Debug, Copy)]
pub struct Then<T, U, B: ?Sized> {
    left: T,
    right: U,
    _marker: PhantomData<B>,
}

impl<T, U, B: ?Sized> Then<T, U, B> {
    /// Compose two lenses
    ///
    /// See also `LensExt::then`.
    pub fn new<A: ?Sized, C: ?Sized>(left: T, right: U) -> Self
    where
        T: Lens<A, B>,
        U: Lens<B, C>,
    {
        Self {
            left,
            right,
            _marker: PhantomData,
        }
    }
}

impl<T, U, A, B, C> Lens<A, C> for Then<T, U, B>
where
    A: ?Sized,
    B: ?Sized,
    C: ?Sized,
    T: Lens<A, B>,
    U: Lens<B, C>,
{
    fn with<V, F: FnOnce(&C) -> V>(&self, data: &A, f: F) -> V {
        self.left.with(data, |b| self.right.with(b, f))
    }

    fn with_mut<V, F: FnOnce(&mut C) -> V>(&self, data: &mut A, f: F) -> V {
        self.left.with_mut(data, |b| self.right.with_mut(b, f))
    }
}

impl<T: Clone, U: Clone, B> Clone for Then<T, U, B> {
    fn clone(&self) -> Self {
        Self {
            left: self.left.clone(),
            right: self.right.clone(),
            _marker: PhantomData,
        }
    }
}

/// `Lens` built from a getter and a setter
#[derive(Debug, Copy, Clone)]
pub struct Map<Get, Put> {
    get: Get,
    put: Put,
}

impl<Get, Put> Map<Get, Put> {
    /// Construct a mapping
    ///
    /// See also `LensExt::map`
    pub fn new<A: ?Sized, B>(get: Get, put: Put) -> Self
    where
        Get: Fn(&A) -> B,
        Put: Fn(&mut A, B),
    {
        Self { get, put }
    }
}

impl<A: ?Sized, B, Get, Put> Lens<A, B> for Map<Get, Put>
where
    Get: Fn(&A) -> B,
    Put: Fn(&mut A, B),
{
    fn with<V, F: FnOnce(&B) -> V>(&self, data: &A, f: F) -> V {
        f(&(self.get)(data))
    }

    fn with_mut<V, F: FnOnce(&mut B) -> V>(&self, data: &mut A, f: F) -> V {
        let mut temp = (self.get)(data);
        let x = f(&mut temp);
        (self.put)(data, temp);
        x
    }
}

/// `Lens` for invoking `Deref` and `DerefMut` on a type
///
/// See also `LensExt::deref`.
#[derive(Debug, Copy, Clone)]
pub struct Deref;

impl<T: ?Sized> Lens<T, T::Target> for Deref
where
    T: ops::Deref + ops::DerefMut,
{
    fn with<V, F: FnOnce(&T::Target) -> V>(&self, data: &T, f: F) -> V {
        f(data.deref())
    }
    fn with_mut<V, F: FnOnce(&mut T::Target) -> V>(&self, data: &mut T, f: F) -> V {
        f(data.deref_mut())
    }
}

/// `Lens` for indexing containers
#[derive(Debug, Copy, Clone)]
pub struct Index<I> {
    index: I,
}

impl<I> Index<I> {
    /// Construct a lens that accesses a particular index
    ///
    /// See also `LensExt::index`.
    pub fn new(index: I) -> Self {
        Self { index }
    }
}

impl<T, I> Lens<T, T::Output> for Index<I>
where
    T: ?Sized + ops::Index<I> + ops::IndexMut<I>,
    I: Clone,
{
    fn with<V, F: FnOnce(&T::Output) -> V>(&self, data: &T, f: F) -> V {
        f(&data[self.index.clone()])
    }
    fn with_mut<V, F: FnOnce(&mut T::Output) -> V>(&self, data: &mut T, f: F) -> V {
        f(&mut data[self.index.clone()])
    }
}

/// The identity lens: the lens which does nothing, i.e. exposes exactly the original value.
///
/// Useful for starting a lens combinator chain, or passing to lens-based interfaces.
#[derive(Debug, Copy, Clone)]
pub struct Id;

impl<A: ?Sized> Lens<A, A> for Id {
    fn with<V, F: FnOnce(&A) -> V>(&self, data: &A, f: F) -> V {
        f(data)
    }

    fn with_mut<V, F: FnOnce(&mut A) -> V>(&self, data: &mut A, f: F) -> V {
        f(data)
    }
}

/// A `Lens` that exposes data within an `Arc` with copy-on-write semantics
///
/// A copy is only made in the event that a different value is written.
#[derive(Debug, Copy, Clone)]
pub struct InArc<L> {
    inner: L,
}

impl<L> InArc<L> {
    /// Adapt a lens to operate on an `Arc`
    ///
    /// See also `LensExt::in_arc`
    pub fn new<A, B>(inner: L) -> Self
    where
        A: Clone,
        B: Data,
        L: Lens<A, B>,
    {
        Self { inner }
    }
}

impl<A, B, L> Lens<Arc<A>, B> for InArc<L>
where
    A: Clone,
    B: Data,
    L: Lens<A, B>,
{
    fn with<V, F: FnOnce(&B) -> V>(&self, data: &Arc<A>, f: F) -> V {
        self.inner.with(data, f)
    }

    fn with_mut<V, F: FnOnce(&mut B) -> V>(&self, data: &mut Arc<A>, f: F) -> V {
        let mut temp = self.inner.with(data, |x| x.clone());
        let v = f(&mut temp);
        if self.inner.with(data, |x| !x.same(&temp)) {
            self.inner.with_mut(Arc::make_mut(data), |x| *x = temp);
        }
        v
    }
}
