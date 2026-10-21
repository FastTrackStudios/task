---
kind: schema
for: session
extends: node
properties:
  - key: room
    display_name: Room
    ty:
      type: enum_with_metadata
      options:
        - { value: a-room, label: A room, order: 0 }
        - { value: b-room, label: B room, order: 1 }
  - key: engineer
    display_name: Engineer
    ty: { type: text }
  - key: date
    display_name: Date
    ty: { type: date }
  - key: booked_hours
    display_name: Booked hours
    ty: { type: number }
---

# Session schema

Declares the `session` kind for this org. A note with `kind: session`
is held to these properties whenever an agent writes it through
`update_properties` or `write_note`: the room has to be one of the two
rooms, the date a date, and the booked hours a number.
`read_property_schema` lists this kind alongside the built-in ones.
