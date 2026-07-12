//! Trigger-volume PRL section (ID 44). Invisible brush AABBs and declarative mover commands.

use crate::FormatError;

pub const TRIGGER_VOLUMES_VERSION: u16 = 2;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TriggerVolumesSection {
    pub triggers: Vec<TriggerVolumeRecord>,
}

/// Field order is persistent wire layout. Do not reorder.
#[derive(Debug, Clone, PartialEq)]
pub struct TriggerVolumeRecord {
    pub name: String,
    pub tags: Vec<String>,
    pub aabb_min: [f32; 3],
    pub aabb_max: [f32; 3],
    pub activation: u8,
    pub target_tag: String,
    pub command: u8,
    pub command_arg: String,
    pub fire_mode: u8,
    pub rearm_ms: f32,
    pub enabled_on_spawn: bool,
    // Appended in v2. Do not move before the v1 fields above.
    pub on_fire: String,
    pub on_exit: String,
}

impl TriggerVolumesSection {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&TRIGGER_VOLUMES_VERSION.to_le_bytes());
        count(&mut out, self.triggers.len());
        for trigger in &self.triggers {
            string(&mut out, &trigger.name);
            count(&mut out, trigger.tags.len());
            for tag in &trigger.tags {
                string(&mut out, tag);
            }
            vec3(&mut out, trigger.aabb_min);
            vec3(&mut out, trigger.aabb_max);
            out.push(trigger.activation);
            string(&mut out, &trigger.target_tag);
            out.push(trigger.command);
            string(&mut out, &trigger.command_arg);
            out.push(trigger.fire_mode);
            out.extend_from_slice(&trigger.rearm_ms.to_le_bytes());
            out.push(u8::from(trigger.enabled_on_spawn));
            string(&mut out, &trigger.on_fire);
            string(&mut out, &trigger.on_exit);
        }
        out
    }

    pub fn from_bytes(data: &[u8]) -> crate::Result<Self> {
        let mut o = 0;
        let version = u16_(&mut o, data, "version")?;
        let has_event_names = match version {
            1 => false,
            TRIGGER_VOLUMES_VERSION => true,
            _ => return invalid(format!("trigger volumes: unsupported version {version}")),
        };
        let n = count_(&mut o, data, "trigger count")?;
        let mut triggers = Vec::with_capacity(n);
        for i in 0..n {
            let name = string_(&mut o, data, &format!("trigger {i} name"))?;
            let tags_n = count_(&mut o, data, &format!("trigger {i} tag count"))?;
            let mut tags = Vec::with_capacity(tags_n);
            for j in 0..tags_n {
                tags.push(string_(&mut o, data, &format!("trigger {i} tag {j}"))?);
            }
            let aabb_min = vec3_(&mut o, data, &format!("trigger {i} min"))?;
            let aabb_max = vec3_(&mut o, data, &format!("trigger {i} max"))?;
            if !aabb_min
                .iter()
                .chain(aabb_max.iter())
                .all(|v| v.is_finite())
            {
                return invalid(format!("trigger volumes: trigger {i} AABB is non-finite"));
            }
            if aabb_min.iter().zip(aabb_max).any(|(min, max)| min > &max) {
                return invalid(format!("trigger volumes: trigger {i} AABB min exceeds max"));
            }
            let activation = u8_(&mut o, data, &format!("trigger {i} activation"))?;
            if activation > 1 {
                return invalid(format!(
                    "trigger volumes: trigger {i} has invalid activation {activation}"
                ));
            }
            let target_tag = string_(&mut o, data, &format!("trigger {i} target tag"))?;
            let command = u8_(&mut o, data, &format!("trigger {i} command"))?;
            if command > 3 {
                return invalid(format!(
                    "trigger volumes: trigger {i} has invalid command {command}"
                ));
            }
            let command_arg = string_(&mut o, data, &format!("trigger {i} command arg"))?;
            let fire_mode = u8_(&mut o, data, &format!("trigger {i} fire mode"))?;
            if fire_mode > 1 {
                return invalid(format!(
                    "trigger volumes: trigger {i} has invalid fire mode {fire_mode}"
                ));
            }
            let rearm_ms = f32_(&mut o, data, &format!("trigger {i} rearm"))?;
            if !rearm_ms.is_finite() || rearm_ms < 0.0 {
                return invalid(format!(
                    "trigger volumes: trigger {i} rearm_ms must be finite and non-negative"
                ));
            }
            let enabled_on_spawn = match u8_(&mut o, data, &format!("trigger {i} enabled"))? {
                0 => false,
                1 => true,
                v => {
                    return invalid(format!(
                        "trigger volumes: trigger {i} has invalid enabled byte {v}"
                    ));
                }
            };
            let (on_fire, on_exit) = if has_event_names {
                (
                    string_(&mut o, data, &format!("trigger {i} on_fire"))?,
                    string_(&mut o, data, &format!("trigger {i} on_exit"))?,
                )
            } else {
                (String::new(), String::new())
            };
            triggers.push(TriggerVolumeRecord {
                name,
                tags,
                aabb_min,
                aabb_max,
                activation,
                target_tag,
                command,
                command_arg,
                fire_mode,
                rearm_ms,
                enabled_on_spawn,
                on_fire,
                on_exit,
            });
        }
        if o != data.len() {
            return invalid(format!(
                "trigger volumes: trailing bytes: expected {o}, got {}",
                data.len()
            ));
        }
        Ok(Self { triggers })
    }
}

fn invalid<T>(message: String) -> crate::Result<T> {
    Err(FormatError::Io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    )))
}
fn count(out: &mut Vec<u8>, n: usize) {
    out.extend_from_slice(&(n as u32).to_le_bytes());
}
fn string(out: &mut Vec<u8>, s: &str) {
    count(out, s.len());
    out.extend_from_slice(s.as_bytes());
}
fn vec3(out: &mut Vec<u8>, value: [f32; 3]) {
    for v in value {
        out.extend_from_slice(&v.to_le_bytes());
    }
}
fn take<'a>(o: &mut usize, data: &'a [u8], n: usize, what: &str) -> crate::Result<&'a [u8]> {
    let end = o
        .checked_add(n)
        .filter(|end| *end <= data.len())
        .ok_or_else(|| {
            FormatError::Io(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("trigger volumes: truncated {what}"),
            ))
        })?;
    let slice = &data[*o..end];
    *o = end;
    Ok(slice)
}
fn u8_(o: &mut usize, d: &[u8], what: &str) -> crate::Result<u8> {
    Ok(take(o, d, 1, what)?[0])
}
fn u16_(o: &mut usize, d: &[u8], what: &str) -> crate::Result<u16> {
    let b = take(o, d, 2, what)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}
fn u32_(o: &mut usize, d: &[u8], what: &str) -> crate::Result<u32> {
    let b = take(o, d, 4, what)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn f32_(o: &mut usize, d: &[u8], what: &str) -> crate::Result<f32> {
    Ok(f32::from_bits(u32_(o, d, what)?))
}
fn count_(o: &mut usize, d: &[u8], what: &str) -> crate::Result<usize> {
    Ok(u32_(o, d, what)? as usize)
}
fn vec3_(o: &mut usize, d: &[u8], what: &str) -> crate::Result<[f32; 3]> {
    Ok([f32_(o, d, what)?, f32_(o, d, what)?, f32_(o, d, what)?])
}
fn string_(o: &mut usize, d: &[u8], what: &str) -> crate::Result<String> {
    let n = count_(o, d, &format!("{what} length"))?;
    let b = take(o, d, n, what)?;
    std::str::from_utf8(b).map(str::to_owned).map_err(|_| {
        FormatError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("trigger volumes: invalid UTF-8 in {what}"),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> TriggerVolumesSection {
        TriggerVolumesSection {
            triggers: vec![TriggerVolumeRecord {
                name: "pad".into(),
                tags: vec!["event".into()],
                aabb_min: [-1.0, 2.0, 3.0],
                aabb_max: [4.0, 5.0, 6.0],
                activation: 0,
                target_tag: "lift".into(),
                command: 3,
                command_arg: "top".into(),
                fire_mode: 1,
                rearm_ms: 200.0,
                enabled_on_spawn: true,
                on_fire: "open_lift".into(),
                on_exit: "close_lift".into(),
            }],
        }
    }

    fn v1_bytes() -> Vec<u8> {
        let trigger = &sample().triggers[0];
        let mut out = Vec::new();
        out.extend_from_slice(&1_u16.to_le_bytes());
        count(&mut out, 1);
        string(&mut out, &trigger.name);
        count(&mut out, trigger.tags.len());
        for tag in &trigger.tags {
            string(&mut out, tag);
        }
        vec3(&mut out, trigger.aabb_min);
        vec3(&mut out, trigger.aabb_max);
        out.push(trigger.activation);
        string(&mut out, &trigger.target_tag);
        out.push(trigger.command);
        string(&mut out, &trigger.command_arg);
        out.push(trigger.fire_mode);
        out.extend_from_slice(&trigger.rearm_ms.to_le_bytes());
        out.push(u8::from(trigger.enabled_on_spawn));
        out
    }

    #[test]
    fn round_trip_preserves_persistent_field_order() {
        let section = sample();
        assert_eq!(
            TriggerVolumesSection::from_bytes(&section.to_bytes()).unwrap(),
            section
        );
    }

    #[test]
    fn v2_appends_event_names_after_the_v1_layout() {
        let mut v2 = sample().to_bytes();
        v2[..2].copy_from_slice(&1_u16.to_le_bytes());
        let v1 = v1_bytes();
        assert_eq!(&v2[..v1.len()], v1);
    }

    #[test]
    fn v1_decode_defaults_event_names_to_empty() {
        let mut expected = sample();
        expected.triggers[0].on_fire.clear();
        expected.triggers[0].on_exit.clear();
        assert_eq!(
            TriggerVolumesSection::from_bytes(&v1_bytes()).unwrap(),
            expected
        );
    }

    #[test]
    fn rejects_invalid_enabled_byte() {
        let mut bytes = v1_bytes();
        *bytes.last_mut().unwrap() = 2;
        assert!(TriggerVolumesSection::from_bytes(&bytes).is_err());
    }

    #[test]
    fn rejects_trailing_bytes_for_v1_and_v2() {
        let mut v1 = v1_bytes();
        v1.push(0);
        assert!(TriggerVolumesSection::from_bytes(&v1).is_err());

        let mut v2 = sample().to_bytes();
        v2.push(0);
        assert!(TriggerVolumesSection::from_bytes(&v2).is_err());
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut bytes = sample().to_bytes();
        bytes[..2].copy_from_slice(&3_u16.to_le_bytes());
        assert!(TriggerVolumesSection::from_bytes(&bytes).is_err());
    }
}
