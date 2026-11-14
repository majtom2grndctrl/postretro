"""Clip events for mixamo_to_gltf.py: the --event flag, its checks, and the events trims keep.
See: context/lib/resource_management.md §7"""

import re

# Case-sensitive, as descriptors name an event by string.
EVENT_NAME = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")

# Times this close match, as trims round their cuts to keys.
KEY_TOLERANCE = 1e-4


def parse_event(spec):
    """Parse `SOURCE:NAME=MS` into (source, name, ms)."""
    number = r"[0-9]+(?:\.[0-9]+)?"
    match = re.fullmatch(rf"([^=:]+):([^=:]+)=({number})", spec)
    if not match:
        raise ValueError(f"--event '{spec}': expected SOURCE:NAME=MS")
    source, name, ms = match.group(1), match.group(2), float(match.group(3))
    check_event_name(name, f"--event '{spec}'")
    return source, name, ms


def check_event_name(name, context):
    if not isinstance(name, str) or not EVENT_NAME.fullmatch(name):
        raise ValueError(f"{context}: event name '{name}' must start with a letter or "
                         "underscore and hold only letters, digits and underscores")


def cut_key(track_times, at):
    """Return the latest key of any track at or before `at` seconds, or None.

    A trim rounds its start and its end down to this key.
    """
    return max((t for times in track_times for t in times if t <= at + KEY_TOLERANCE),
               default=None)


def clip_end(track_times):
    return max((times[-1] for times in track_times if times), default=0.0)


def clip_events(animation):
    """Return a clip's `extras.events`, empty when it has none.

    Raises ValueError for extras that are not an object or events that are not a
    list of `{ name, time }` objects.
    """
    clip = animation.get("name")
    extras = animation.get("extras", {})
    if not isinstance(extras, dict):
        raise ValueError(f"clip '{clip}': extras is not an object")
    events = extras.get("events", [])
    if not isinstance(events, list) or not all(
            isinstance(event, dict) and isinstance(event.get("name"), str)
            and isinstance(event.get("time"), (int, float))
            and not isinstance(event.get("time"), bool)
            for event in events):
        raise ValueError(f"clip '{clip}': extras.events is not a list of {{ name, time }}")
    return events


def set_clip_events(animation, events):
    """Write `events` as a clip's `extras.events`, keeping its other extras.

    Writes nothing for an empty list.
    """
    if events:
        animation.setdefault("extras", {})["events"] = events


def add_event(animation, name, ms, track_times, context):
    """Append event `name` at `ms` milliseconds to a clip.

    The time must lie within the clip, from 0 to its last key, and the name must
    be new to the clip. `track_times` lists each of the clip's tracks' key times
    in seconds.
    """
    check_event_name(name, context)
    clip = animation.get("name")
    time = ms / 1000.0
    end = clip_end(track_times)
    if time < 0 or time > end + KEY_TOLERANCE:
        raise ValueError(f"{context}: {ms:g} ms is outside '{clip}' "
                         f"(0 to {end * 1000:.0f} ms)")
    events = clip_events(animation)
    if any(event["name"] == name for event in events):
        raise ValueError(f"{context}: '{clip}' already has an event named '{name}'")
    set_clip_events(animation, events + [{"name": name, "time": min(time, end)}])


def find_animation(gltf, name, context):
    for animation in gltf.get("animations", []):
        if animation.get("name") == name:
            return animation
    raise ValueError(f"{context}: no clip named '{name}'")


def attach_events(gltf, track_times, events):
    """Attach each (source, name, ms) event to its source clip, in order.

    `track_times` maps each clip name to its tracks' key times in seconds.
    """
    for source, name, ms in events:
        context = f"--event {source}:{name}={ms:g}"
        animation = find_animation(gltf, source, context)
        add_event(animation, name, ms, track_times[source], context)


def carried_events(events, start, end, to_clip_end):
    """Return the events a cut from `start` to `end` seconds keeps, shifted to zero.

    `start` and `end` are the cut's actual keys, its requested times rounded down
    to a key, so an event shifts by the same amount as the cut's keys. An event
    at a requested start that falls between keys lands at its offset from the key
    the start rounds down to.

    The kept span is half-open, [start, end): an event on the key where a windup
    ends and its strike starts belongs to the strike, at 0, not to the windup. A
    windup therefore keeps only events before its last key. `to_clip_end` closes
    the span when the cut keeps the source's last key, so an event there survives.
    Times within KEY_TOLERANCE of a key count as on it.

    Carried times round to the microsecond. An `end` field shifts the same way and
    clamps to the cut. Other fields copy unchanged.
    """
    duration = end - start

    def shift(time):
        return min(max(round(time - start, 6), 0.0), duration)

    kept = []
    for event in events:
        time = event["time"]
        if time < start - KEY_TOLERANCE or time > end + KEY_TOLERANCE:
            continue
        if not to_clip_end and time >= end - KEY_TOLERANCE:
            continue
        carried = dict(event, time=shift(time))
        span_end = event.get("end")
        if isinstance(span_end, (int, float)) and not isinstance(span_end, bool):
            carried["end"] = shift(span_end)
        kept.append(carried)
    return kept


def trim_events(source, track_times, start_ms, end_ms, context):
    """Return the events a `--trim` of clip `source` keeps.

    The start and end round down to a key as trim_clip rounds them: a start of 0
    is the source's first moment and an end of None its last key. `track_times`
    lists each of the source's tracks' key times in seconds.
    """
    last = clip_end(track_times)
    start = cut_key(track_times, start_ms / 1000.0) if start_ms else 0.0
    end = last if end_ms is None else cut_key(track_times, end_ms / 1000.0)
    if start is None or end is None:
        raise ValueError(f"{context}: the cut starts or ends before the first key")
    return carried_events(clip_events(source), start, end, end >= last - KEY_TOLERANCE)


def stamp_library_events(gltf, track_times, events, trims=()):
    """Add events to an exported document whose trimmed clips already exist.

    Attaches each (source, name, ms) event as `--event` does, then gives each
    trim's clip the events its cut keeps from its source. `trims` are parse_trim
    tuples in build order, so a trim of a trim keeps what its source kept.
    `track_times` maps each clip name to its tracks' key times in seconds; read
    them with mixamo_gltf_post.document_track_times.

    When the trimmed clips carry no events yet, the result matches a re-export
    with the same `--event` and `--trim` flags.
    """
    attach_events(gltf, track_times, events)
    for new, source, start_ms, end_ms in trims:
        context = f"--trim {new}"
        carried = trim_events(find_animation(gltf, source, context), track_times[source],
                              start_ms, end_ms, context)
        clip = find_animation(gltf, new, context)
        existing = clip_events(clip)
        clash = {event["name"] for event in existing} & {event["name"] for event in carried}
        if clash:
            raise ValueError(f"{context}: '{new}' already has events named {sorted(clash)}")
        set_clip_events(clip, existing + carried)
