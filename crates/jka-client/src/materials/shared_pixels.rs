//! Content-interned pixel allocations. Pools hold weak references only;
//! mutation detaches, so independent material/lighting edits stay independent.
use std::{collections::HashMap, hash::{Hash, Hasher}, ops::{Deref, DerefMut}, sync::{Arc, Mutex, OnceLock, Weak}};

pub trait Pixel: Clone + Eq + Hash + std::fmt::Debug + Send + Sync + 'static {
    fn intern(pixels: Arc<Vec<Self>>) -> Arc<Vec<Self>>;
}

#[derive(Default)]
struct Pool<T> {
    entries: HashMap<u64, Vec<Weak<Vec<T>>>>,
    misses: usize,
}

fn intern<T: Pixel>(pixels: Arc<Vec<T>>, pool: &Mutex<Pool<T>>) -> Arc<Vec<T>> {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    pixels.hash(&mut hash);
    let key = hash.finish();
    let mut pool = pool.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(bucket) = pool.entries.get(&key) {
        for previous in bucket.iter().filter_map(Weak::upgrade) {
            // Hashes select a bucket; exact content decides equivalence.
            if *previous == *pixels { return previous; }
        }
    }
    pool.misses += 1;
    if pool.misses >= 64 {
        pool.entries.retain(|_, bucket| {
            bucket.retain(|entry| entry.strong_count() != 0);
            !bucket.is_empty()
        });
        pool.misses = 0;
    }
    pool.entries.entry(key).or_default().push(Arc::downgrade(&pixels));
    pixels
}

macro_rules! pixel {
    ($ty:ty) => {
        impl Pixel for $ty {
            fn intern(pixels: Arc<Vec<Self>>) -> Arc<Vec<Self>> {
                static POOL: OnceLock<Mutex<Pool<$ty>>> = OnceLock::new();
                intern(pixels, POOL.get_or_init(|| Mutex::new(Pool::default())))
            }
        }
    };
}
pixel!(u8);
pixel!(u16);

#[derive(Clone, Debug)]
pub struct SharedPixels<T: Pixel>(Arc<Vec<T>>, bool);

impl<T: Pixel> SharedPixels<T> {
    pub(crate) fn interned(&self) -> Self {
        if self.1 { self.clone() } else { Self(T::intern(Arc::clone(&self.0)), true) }
    }
    pub(crate) fn identity(&self) -> usize { Arc::as_ptr(&self.0) as usize }
    pub(crate) fn ptr_eq(&self, other: &Self) -> bool { Arc::ptr_eq(&self.0, &other.0) }
    pub fn into_vec(self) -> Vec<T> { Arc::try_unwrap(self.0).unwrap_or_else(|p| p.as_ref().clone()) }
}

impl<T: Pixel> From<Vec<T>> for SharedPixels<T> {
    fn from(mut pixels: Vec<T>) -> Self {
        if pixels.capacity() > pixels.len().saturating_add(pixels.len() / 8) { pixels.shrink_to_fit(); }
        Self(T::intern(Arc::new(pixels)), true)
    }
}
impl<T: Pixel> Default for SharedPixels<T> { fn default() -> Self { Vec::new().into() } }
impl<T: Pixel> Deref for SharedPixels<T> { type Target = Vec<T>; fn deref(&self) -> &Vec<T> { &self.0 } }
impl<T: Pixel> DerefMut for SharedPixels<T> { fn deref_mut(&mut self) -> &mut Vec<T> { self.1 = false; Arc::make_mut(&mut self.0) } }
impl<T: Pixel> PartialEq for SharedPixels<T> { fn eq(&self, other: &Self) -> bool { self.0 == other.0 } }
impl<T: Pixel> Eq for SharedPixels<T> {}
impl<T: Pixel> AsRef<[T]> for SharedPixels<T> { fn as_ref(&self) -> &[T] { &self.0 } }
impl<T: Pixel> FromIterator<T> for SharedPixels<T> { fn from_iter<I: IntoIterator<Item=T>>(iter: I) -> Self { Vec::from_iter(iter).into() } }
impl<T: Pixel> IntoIterator for SharedPixels<T> { type Item=T; type IntoIter=std::vec::IntoIter<T>; fn into_iter(self) -> Self::IntoIter { self.into_vec().into_iter() } }
impl<'a, T: Pixel> IntoIterator for &'a SharedPixels<T> { type Item=&'a T; type IntoIter=std::slice::Iter<'a,T>; fn into_iter(self) -> Self::IntoIter { self.iter() } }
impl<'a, T: Pixel> IntoIterator for &'a mut SharedPixels<T> { type Item=&'a mut T; type IntoIter=std::slice::IterMut<'a,T>; fn into_iter(self) -> Self::IntoIter { self.iter_mut() } }
impl<T: Pixel> PartialEq<Vec<T>> for SharedPixels<T> { fn eq(&self, other: &Vec<T>) -> bool { self.0.as_ref() == other } }
impl<T: Pixel> PartialEq<SharedPixels<T>> for Vec<T> { fn eq(&self, other: &SharedPixels<T>) -> bool { self == other.0.as_ref() } }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_assets_share_pixels_and_edits_detach() {
        let a: SharedPixels<u8> = vec![11, 22, 33, 255].into();
        let mut b: SharedPixels<u8> = vec![11, 22, 33, 255].into();
        assert!(a.ptr_eq(&b));
        b[0] = 99;
        assert!(!a.ptr_eq(&b));
        assert_eq!(a[0], 11);
        assert_eq!(b[0], 99);
        let c: SharedPixels<u8> = vec![11, 22, 33, 255].into();
        assert!(a.ptr_eq(&c));
        let x: SharedPixels<u16> = vec![0x3c00, 0x4000].into();
        let y: SharedPixels<u16> = vec![0x3c00, 0x4000].into();
        assert!(x.ptr_eq(&y));
    }
    #[test]
    fn pool_does_not_retain_unused_assets() {
        let pixels: SharedPixels<u8> = vec![8, 19, 71].into();
        let weak = Arc::downgrade(&pixels.0);
        drop(pixels);
        assert!(weak.upgrade().is_none());
    }
}
