//! `lib/offsets.js` and `lib/remap-bitfield.js`.

use std::cmp::Ordering;

use super::util::Screen;

/// An arbitrary-precision non-negative bit set (JS `bigint` used as a bitfield).
#[derive(Clone, Default, PartialEq, Eq, Hash, Debug)]
pub struct Bits(Vec<u64>);

impl Bits {
    pub fn zero() -> Bits {
        Bits(Vec::new())
    }

    pub fn bit(n: u64) -> Bits {
        let mut v = vec![0u64; (n / 64) as usize + 1];
        v[(n / 64) as usize] = 1 << (n % 64);
        Bits(v)
    }

    fn trim(mut self) -> Bits {
        while self.0.last() == Some(&0) {
            self.0.pop();
        }
        self
    }

    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|w| *w == 0)
    }

    pub fn or(&self, o: &Bits) -> Bits {
        let n = self.0.len().max(o.0.len());
        Bits((0..n).map(|i| self.0.get(i).copied().unwrap_or(0) | o.0.get(i).copied().unwrap_or(0)).collect()).trim()
    }

    pub fn and(&self, o: &Bits) -> Bits {
        let n = self.0.len().min(o.0.len());
        Bits((0..n).map(|i| self.0[i] & o.0[i]).collect()).trim()
    }

    /// `self & ~o`
    pub fn and_not(&self, o: &Bits) -> Bits {
        Bits((0..self.0.len()).map(|i| self.0[i] & !o.0.get(i).copied().unwrap_or(0)).collect()).trim()
    }

    pub fn shl(&self, n: u64) -> Bits {
        if self.is_zero() {
            return Bits::zero();
        }
        let words = (n / 64) as usize;
        let bits = n % 64;
        let mut v = vec![0u64; self.0.len() + words + 1];
        for (i, w) in self.0.iter().enumerate() {
            v[i + words] |= w << bits;
            if bits > 0 {
                v[i + words + 1] |= w >> (64 - bits);
            }
        }
        Bits(v).trim()
    }

    /// `self - 1` for `self > 0`
    fn minus_one(&self) -> Bits {
        let mut v = self.0.clone();
        for w in v.iter_mut() {
            if *w == 0 {
                *w = u64::MAX;
            } else {
                *w -= 1;
                break;
            }
        }
        Bits(v).trim()
    }

    /// `v & ~(m | (m - 1n))` (with `m = 0n` → `0n`)
    pub fn mask_after(v: &Bits, m: &Bits) -> Bits {
        if m.is_zero() {
            // ~(0 | -1) = 0
            return Bits::zero();
        }
        let mm = m.or(&m.minus_one());
        v.and_not(&mm)
    }

    pub fn cmp_num(&self, o: &Bits) -> Ordering {
        let a = self.clone().trim();
        let b = o.clone().trim();
        a.0.len().cmp(&b.0.len()).then_with(|| {
            for i in (0..a.0.len()).rev() {
                match a.0[i].cmp(&b.0[i]) {
                    Ordering::Equal => continue,
                    o => return o,
                }
            }
            Ordering::Equal
        })
    }

    pub fn contains(&self, bit: &Bits) -> bool {
        !self.and(bit).is_zero()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Layer {
    Defaults,
    Base,
    Components,
    Utilities,
    User,
    Variants,
}

impl Layer {
    pub fn position(self) -> i64 {
        match self {
            Layer::Defaults => 0,
            Layer::Base => 1,
            Layer::Components => 2,
            Layer::Utilities => 3,
            Layer::User => 4,
            Layer::Variants => 5,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Layer::Defaults => "defaults",
            Layer::Base => "base",
            Layer::Components => "components",
            Layer::Utilities => "utilities",
            Layer::User => "user",
            Layer::Variants => "variants",
        }
    }
}

/// `matchVariant` / `addVariant` id
#[derive(Clone, Debug, PartialEq)]
pub enum VariantId {
    Num(u32),
    Str(String),
}

/// A value a variant sort function compares (`{ value, modifier }`).
#[derive(Clone, Debug)]
pub enum SortValue {
    Str(String),
    Screen(Screen),
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SortFn {
    MinScreens,
    MaxScreens,
}

/// `VariantOption` (the `args` object merged with the variant's options)
#[derive(Clone, Debug)]
pub struct VariantOption {
    pub id: Option<VariantId>,
    pub sort: Option<SortFn>,
    pub value: SortValue,
    pub variant: Bits,
}

#[derive(Clone, Debug)]
pub struct RuleOffset {
    pub layer: Layer,
    pub parent_layer: Layer,
    pub arbitrary: bool,
    pub variants: Bits,
    pub parallel_index: u64,
    pub index: u64,
    pub property_offset: u64,
    pub property: String,
    pub options: Vec<VariantOption>,
}

#[derive(Default)]
pub struct Offsets {
    offsets: [u64; 6],
    reserved_variant_bits: u64,
    pub variant_offsets: Vec<(String, Bits)>,
}

fn layer_idx(l: Layer) -> usize {
    l.position() as usize
}

impl Offsets {
    pub fn new() -> Offsets {
        Offsets::default()
    }

    pub fn create(&mut self, layer: Layer) -> RuleOffset {
        let i = layer_idx(layer);
        let index = self.offsets[i];
        self.offsets[i] += 1;
        RuleOffset {
            layer,
            parent_layer: layer,
            arbitrary: false,
            variants: Bits::zero(),
            parallel_index: 0,
            index,
            property_offset: 0,
            property: String::new(),
            options: Vec::new(),
        }
    }

    pub fn arbitrary_property(&mut self, name: &str) -> RuleOffset {
        let mut o = self.create(Layer::Utilities);
        o.arbitrary = true;
        o.property = name.to_string();
        o
    }

    pub fn variant_offset(&self, variant: &str) -> Option<&Bits> {
        self.variant_offsets.iter().find(|(k, _)| k == variant).map(|(_, v)| v)
    }

    pub fn for_variant(&mut self, variant: &str, index: u64) -> RuleOffset {
        let offset = self.variant_offset(variant).cloned().expect("unknown variant");
        let mut o = self.create(Layer::Variants);
        o.variants = offset.shl(index);
        o
    }

    pub fn apply_variant_offset(&self, rule: &RuleOffset, variant: &RuleOffset, options: VariantOption) -> RuleOffset {
        let mut options = options;
        options.variant = variant.variants.clone();
        let mut new_options = rule.options.clone();
        if options.sort.is_some() {
            new_options.insert(0, options);
        }
        RuleOffset {
            layer: Layer::Variants,
            parent_layer: if rule.layer == Layer::Variants { rule.parent_layer } else { rule.layer },
            variants: rule.variants.or(&variant.variants),
            options: new_options,
            parallel_index: rule.parallel_index.max(variant.parallel_index),
            ..rule.clone()
        }
    }

    pub fn apply_parallel_offset(&self, offset: &RuleOffset, parallel_index: u64) -> RuleOffset {
        RuleOffset { parallel_index, ..offset.clone() }
    }

    pub fn record_variant(&mut self, variant: &str, fn_count: u64) -> RuleOffset {
        let bit = Bits::bit(self.reserved_variant_bits);
        if let Some(e) = self.variant_offsets.iter_mut().find(|(k, _)| k == variant) {
            e.1 = bit.clone();
        } else {
            self.variant_offsets.push((variant.to_string(), bit.clone()));
        }
        self.reserved_variant_bits += fn_count;
        let mut o = self.create(Layer::Variants);
        o.variants = bit;
        o
    }

    pub fn compare(&self, a: &RuleOffset, b: &RuleOffset) -> Ordering {
        if a.layer != b.layer {
            return a.layer.position().cmp(&b.layer.position());
        }
        if a.parent_layer != b.parent_layer {
            return a.parent_layer.position().cmp(&b.parent_layer.position());
        }
        for ao in &a.options {
            for bo in &b.options {
                if ao.id != bo.id {
                    continue;
                }
                let (Some(asort), Some(_)) = (ao.sort, bo.sort) else { continue };
                let max_fn_variant = if ao.variant.cmp_num(&bo.variant) == Ordering::Greater { &ao.variant } else { &bo.variant };
                let av = Bits::mask_after(&a.variants, max_fn_variant);
                let bv = Bits::mask_after(&b.variants, max_fn_variant);
                if av != bv {
                    continue;
                }
                let r = sort_fn(asort, &ao.value, &bo.value);
                // `bigSign(NaN)` is 0
                if !r.is_nan() && r != 0.0 {
                    return if r < 0.0 { Ordering::Less } else { Ordering::Greater };
                }
                if r.is_nan() {
                    // `if (result !== 0) return result` returns NaN, which bigSign maps to 0
                    return Ordering::Equal;
                }
            }
        }
        if a.variants != b.variants {
            return a.variants.cmp_num(&b.variants);
        }
        if a.parallel_index != b.parallel_index {
            return a.parallel_index.cmp(&b.parallel_index);
        }
        if a.arbitrary != b.arbitrary {
            return a.arbitrary.cmp(&b.arbitrary);
        }
        if a.property_offset != b.property_offset {
            return a.property_offset.cmp(&b.property_offset);
        }
        a.index.cmp(&b.index)
    }

    fn recalculate_variant_offsets(&self) -> Vec<(Bits, Bits)> {
        let mut variants: Vec<(String, Bits)> = self.variant_offsets.iter().filter(|(v, _)| v.starts_with('[')).cloned().collect();
        variants.sort_by(|(a, _), (z, _)| crate::js::cmp_utf16(a, z));
        let mut new_offsets: Vec<Bits> = variants.iter().map(|(_, o)| o.clone()).collect();
        new_offsets.sort_by(|a, z| a.cmp_num(z));
        variants
            .into_iter()
            .zip(new_offsets)
            .map(|((_, old), new)| (old, new))
            .filter(|(a, z)| a != z)
            .collect()
    }

    pub fn sort<T: Clone>(&self, list: Vec<(RuleOffset, T)>) -> Vec<(RuleOffset, T)> {
        // remapArbitraryVariantOffsets
        let mapping = self.recalculate_variant_offsets();
        let mut list = if mapping.is_empty() {
            list
        } else {
            list.into_iter()
                .map(|(mut o, r)| {
                    o.variants = remap_bitfield(&o.variants, &mapping);
                    (o, r)
                })
                .collect()
        };
        // sortArbitraryProperties
        let mut known: Vec<String> = Vec::new();
        for (o, _) in &list {
            if o.arbitrary && !known.contains(&o.property) {
                known.push(o.property.clone());
            }
        }
        if !known.is_empty() {
            known.sort_by(|a, b| crate::js::cmp_utf16(a, b));
            for (o, _) in list.iter_mut() {
                o.property_offset = known.iter().position(|p| *p == o.property).map(|i| i as u64 + 1).unwrap_or(0);
            }
        }
        crate::js::sort_by(&mut list, |(a, _), (b, _)| match self.compare(a, b) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        });
        list
    }
}

pub fn remap_bitfield(num: &Bits, mapping: &[(Bits, Bits)]) -> Bits {
    let mut old_mask = Bits::zero();
    let mut new_mask = Bits::zero();
    for (old, new) in mapping {
        if num.contains(old) {
            old_mask = old_mask.or(old);
            new_mask = new_mask.or(new);
        }
    }
    num.and_not(&old_mask).or(&new_mask)
}

/// `compareScreens(type, a.value, z.value)`
fn sort_fn(f: SortFn, a: &SortValue, z: &SortValue) -> f64 {
    let ty = match f {
        SortFn::MinScreens => "min",
        SortFn::MaxScreens => "max",
    };
    super::screens::compare_screens(ty, a, z)
}
