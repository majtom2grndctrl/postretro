// Audience-routed presentation commands and their local client intake.
// See: context/lib/networking.md §Presentation events vs. replicated state

use glam::Vec3;
use postretro_entities::SystemCommandFireContext;
use postretro_entities::reactions::system_commands::{Recipients, SystemCommandQueue};
use postretro_entities::{ContactHit, Emitter, ImpactContact, SystemReactionCommand};
use postretro_foundation::Seat;
use postretro_net::transport::NetServer;
use postretro_net::wire::{
    PresentationCommand, ServerPresentationMessage, ServerPresentationPayload,
};

use crate::seat::SeatTable;

/// The wire mirror of a presentation command. `None` for any other command.
/// Entity anchors carry the position captured at fire time. Weapon contact
/// anchors stay on their every-machine path and are never forwarded.
pub fn presentation_command_to_wire(
    command: &SystemReactionCommand,
) -> Option<PresentationCommand> {
    Some(match command {
        SystemReactionCommand::PlaySound { sound, bus, at, .. } => PresentationCommand::PlaySound {
            sound: sound.clone(),
            bus: bus.clone(),
            at: match at {
                Some(Emitter::Entity { origin, .. }) => Some(origin.to_array()),
                Some(Emitter::Contacts(_)) => return None,
                None => None,
            },
        },
        SystemReactionCommand::Rumble {
            strong,
            weak,
            duration_ms,
            ..
        } => PresentationCommand::Rumble {
            strong: *strong,
            weak: *weak,
            duration_ms: *duration_ms,
        },
        SystemReactionCommand::FlashScreen {
            color, duration_ms, ..
        } => PresentationCommand::FlashScreen {
            color: *color,
            duration_ms: *duration_ms,
        },
        SystemReactionCommand::Vignette {
            color,
            strength,
            duration_ms,
            ..
        } => PresentationCommand::Vignette {
            color: *color,
            strength: *strength,
            duration_ms: *duration_ms,
        },
        SystemReactionCommand::ScreenShake {
            amplitude,
            duration_ms,
            frequency,
            ..
        } => PresentationCommand::ScreenShake {
            amplitude: *amplitude,
            duration_ms: *duration_ms,
            frequency: *frequency,
        },
        // Listed in full so a new command variant fails to build until it is
        // mapped here or deliberately kept off the wire.
        SystemReactionCommand::PushTree { .. }
        | SystemReactionCommand::LoadLevel { .. }
        | SystemReactionCommand::RestartLevel
        | SystemReactionCommand::ReturnToFrontend
        | SystemReactionCommand::PopTree
        | SystemReactionCommand::SetState { .. }
        | SystemReactionCommand::SetSentiment { .. }
        | SystemReactionCommand::AdjustSentiment { .. }
        | SystemReactionCommand::AddOwnerSlot { .. }
        | SystemReactionCommand::CellWrite { .. }
        | SystemReactionCommand::AppendText { .. }
        | SystemReactionCommand::BackspaceText { .. }
        | SystemReactionCommand::ClearText { .. } => return None,
    })
}

/// The local command a received presentation command becomes: the same one a
/// local reaction enqueues. `None`, with a warning, when a number is not
/// finite.
pub fn presentation_command_from_wire(
    command: PresentationCommand,
) -> Option<SystemReactionCommand> {
    let finite = |values: &[f32]| values.iter().all(|value| value.is_finite());
    let (local, numbers): (SystemReactionCommand, Vec<f32>) = match command {
        PresentationCommand::PlaySound { sound, bus, at } => (
            SystemReactionCommand::PlaySound {
                audience: None,
                sound,
                bus,
                at: at.map(|point| {
                    Emitter::Contacts(vec![ImpactContact {
                        point: Vec3::from_array(point),
                        normal: Vec3::ZERO,
                        hit: ContactHit::World,
                    }])
                }),
            },
            at.into_iter().flatten().collect(),
        ),
        PresentationCommand::Rumble {
            strong,
            weak,
            duration_ms,
        } => (
            SystemReactionCommand::Rumble {
                audience: None,
                strong,
                weak,
                duration_ms,
            },
            [strong, duration_ms].into_iter().chain(weak).collect(),
        ),
        PresentationCommand::FlashScreen { color, duration_ms } => (
            SystemReactionCommand::FlashScreen {
                audience: None,
                color,
                duration_ms,
            },
            color.into_iter().chain([duration_ms]).collect(),
        ),
        PresentationCommand::Vignette {
            color,
            strength,
            duration_ms,
        } => (
            SystemReactionCommand::Vignette {
                audience: None,
                color,
                strength,
                duration_ms,
            },
            [strength, duration_ms]
                .into_iter()
                .chain(color.into_iter().flatten())
                .collect(),
        ),
        PresentationCommand::ScreenShake {
            amplitude,
            duration_ms,
            frequency,
        } => (
            SystemReactionCommand::ScreenShake {
                audience: None,
                amplitude,
                duration_ms,
                frequency,
            },
            [amplitude, duration_ms]
                .into_iter()
                .chain(frequency)
                .collect(),
        ),
    };
    if !finite(&numbers) {
        log::warn!(
            "[Netcode] dropped a forwarded presentation command carrying a non-finite number"
        );
        return None;
    }
    Some(local)
}

/// Resolve the ordered frame queue at the app boundary, retaining local order.
/// Remote commands are disposable cosmetics sent only to participating clients.
pub fn route_player_presentation(
    queue: &SystemCommandQueue,
    server: Option<&mut NetServer>,
    seats: Option<&SeatTable>,
) {
    let ordered = queue.take_ordered();
    if ordered.is_empty() {
        return;
    }
    // A caller may still be inside a scoped fire. Local re-pushes must neither
    // route again nor inherit its audience. Restore that caller's context after.
    let previous = queue.replace_fire_context(SystemCommandFireContext::default());
    let mut server = server;
    for (recipients, mut command) in ordered {
        if recipients == Recipients::Skip {
            continue;
        }
        if recipients == Recipients::Local || server.is_none() {
            command.clear_audience();
            queue.push(command);
            continue;
        }
        let server = server.as_deref_mut().expect("server present above");
        let (local, clients) = match recipients {
            Recipients::Everyone => (true, server.participating_clients()),
            Recipients::Seats(recipients) => {
                let local = recipients.contains(&Seat(0));
                let mut clients = Vec::new();
                for seat in recipients {
                    if seat == Seat(0) {
                        continue;
                    }
                    let Some(client) = seats.and_then(|table| table.client_for_seat(seat)) else {
                        continue;
                    };
                    if server.is_participating(client) && !clients.contains(&client) {
                        clients.push(client);
                    }
                }
                (local, clients)
            }
            Recipients::Local | Recipients::Skip => unreachable!("handled above"),
        };
        if !clients.is_empty()
            && let Some(wire) = presentation_command_to_wire(&command)
        {
            for client in clients {
                let _ = server.send_presentation(
                    client,
                    ServerPresentationMessage {
                        payload: ServerPresentationPayload::Command(wire.clone()),
                    },
                );
            }
        }
        if local {
            command.clear_audience();
            queue.push(command);
        }
    }
    queue.replace_fire_context(previous);
}

/// Split the commands out of a frame's received presentation; the rest go to
/// the passive intake.
pub fn split_presentation_commands(
    messages: Vec<ServerPresentationMessage>,
) -> (Vec<PresentationCommand>, Vec<ServerPresentationMessage>) {
    let mut commands = Vec::new();
    let mut passive = Vec::with_capacity(messages.len());
    for message in messages {
        match message.payload {
            ServerPresentationPayload::Command(command) => commands.push(command),
            payload => passive.push(ServerPresentationMessage { payload }),
        }
    }
    (commands, passive)
}

/// Enqueue received presentation commands as local commands, so this
/// machine's accommodations (flash limiter, reduce motion) apply to them as to
/// its own.
pub fn ingest_presentation_commands(
    commands: Vec<PresentationCommand>,
    queue: &SystemCommandQueue,
) {
    for command in commands {
        if let Some(local) = presentation_command_from_wire(command) {
            queue.push(local);
        }
    }
}

#[cfg(test)]
mod tests;
