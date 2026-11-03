//! Scoring's original generated attribute access paths. Tuning remains in Vault.
use crate::{
    attribute_runtime::{AttributeRuntimeError, AttributeRuntimeInstance},
    physics::PointGraph,
};

#[derive(Debug)]
pub(crate) enum ScoreError {
    Access(crate::physical_player_phys_out::PhysOutAccessError),
    Cvar(crate::input_cvar_runtime::InputCvarError),
    AirReset(super::air_collector::AirResetError),
    Attribute(AttributeRuntimeError),
    Numeric(crate::reckoning_numeric::ReckoningHostNumericError),
    Invalid(&'static str),
    Missing(&'static str),
    Poisoned,
}
impl From<AttributeRuntimeError> for ScoreError {
    fn from(value: AttributeRuntimeError) -> Self {
        Self::Attribute(value)
    }
}
impl std::fmt::Display for ScoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ScoreError {}

pub(super) fn word(instance: &AttributeRuntimeInstance, key: u64) -> Result<u32, ScoreError> {
    let Some(data) = instance.try_get_data(key, 0)? else {
        return Ok(0);
    };
    Ok(u32::from_be_bytes(
        data.bytes()
            .get(..4)
            .ok_or(ScoreError::Invalid("scoring word width"))?
            .try_into()
            .unwrap(),
    ))
}
pub(super) fn layout_word(
    instance: &AttributeRuntimeInstance,
    offset: usize,
) -> Result<u32, ScoreError> {
    let Some(data) = instance.try_cached_layout_field(offset, 4)? else {
        return Ok(0);
    };
    Ok(u32::from_be_bytes(
        data.bytes()
            .try_into()
            .map_err(|_| ScoreError::Invalid("scoring layout word"))?,
    ))
}

/// Generated GetAttributePointer(key,0), followed by DefaultDataArea(4).
pub(super) fn scalar(instance: &AttributeRuntimeInstance, key: u64) -> Result<f32, ScoreError> {
    let Some(data) = instance.try_get_data(key, 0)? else {
        return Ok(0.0);
    };
    let bytes = data
        .bytes()
        .get(..4)
        .ok_or(ScoreError::Invalid("scoring scalar width"))?;
    Ok(f32::from_be_bytes(bytes.try_into().unwrap()))
}

/// PointNegGraphData8 has sixteen metadata bytes, then eight X and eight Y.
/// Native scoring calls PointGraphEval directly; metadata is not a second curve.
pub(super) fn curve(
    instance: &AttributeRuntimeInstance,
    offset: usize,
    x: f32,
) -> Result<f32, ScoreError> {
    let Some(data) = instance.try_cached_layout_field(offset + 16, 64)? else {
        return Ok(PointGraph::<8>::default().evaluate(x));
    };
    let b = data.bytes();
    if b.len() != 64 {
        return Err(ScoreError::Invalid("scoring curve width"));
    }
    let values: [f32; 16] =
        std::array::from_fn(|i| f32::from_be_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()));
    Ok(PointGraph::<8> {
        x: values[..8].try_into().unwrap(),
        y: values[8..].try_into().unwrap(),
    }
    .evaluate(x))
}

// Raw lis/ori/insrdi operands:82df25f0/2640,82dedd68/ddb8,82dea488/668.
pub(super) const LINE_RATE: u64 = 0xfe02_a452_31f7_b06a;
pub(super) const LINE_MAX: u64 = 0x5747_8337_accf_ff18;
pub(super) const MOMENTUM_RATE: u64 = 0x8840_7506_dc97_80ed;
pub(super) const MOMENTUM_MAX: u64 = 0x2876_7a5f_961c_7129;
pub(super) const MOMENTUM_THRESHOLDS: [u64; 3] = [
    0x8298_87d8_c24a_5b8a,
    0xfb84_08a1_5c17_a9d4,
    0x30aa_e071_a786_8771,
];
pub(super) const MOMENTUM_MULTIPLIERS: [u64; 3] = [
    0x4752_0563_64ff_91fe,
    0xf499_3a13_ed7b_44c1,
    0xdc0c_3853_f6c5_fc2e,
];
pub(super) const MOMENTUM_ALLOW_DROP: u64 = 0xe361_97bf_c8ea_1cad;
