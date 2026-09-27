/**
 * The police box in play: its door leaves' mover key. The box's front,
 * the focus on it, the door steps and the walk-in join this module as
 * they are built.
 *
 * A police box's two door leaves are movers (`boxLeafMovers` in
 * `render/models/heroes/street.ts`) that open and close together, so both
 * share one key, named after the hero's place in `room.heroes`: the door
 * state the session keeps under that key turns both leaves at once. The
 * key never collides with a fixture's (`door:<i>`, `lamp:<i>`,
 * `spark:<i>`, `lid:<i>`, `disc:<i>`), since a hero is not a fixture.
 */

/**
 * The mover key of the police box at `index` in `room.heroes`:
 * `box:<index>`, shared by both of its door leaves.
 */
export function boxKey(index: number): string {
  return `box:${String(index)}`;
}
