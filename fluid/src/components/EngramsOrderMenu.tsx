/**
 * Which way the engrams below are listed, chosen from a menu.
 *
 * The trigger says the current choice rather than "Order", because the
 * choice is what a reader scanning the row wants to know, and the four items
 * are the same words. Radix owns the keyboard: arrows between the items,
 * Enter or Space to choose, Escape to leave. The choice is the frame's
 * (`engramsOrder.ts`), so the next folder and the next visit are listed the
 * same way.
 */

import { DropdownMenu } from "radix-ui";

import {
  ENGRAMS_ORDERS,
  isEngramsOrder,
  orderLabel,
  useEngramsOrder,
} from "../engramsOrder";
import { ITEM_CLASSES, MENU_CLASSES } from "./menu";
import { BUTTON } from "./primitives";

export function EngramsOrderMenu() {
  const { order, setOrder } = useEngramsOrder();
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger
        aria-label={`Order: ${orderLabel(order)}`}
        className={`inline-flex items-center gap-1 ${BUTTON.secondary}`}
      >
        <span>{orderLabel(order)}</span>
        <span aria-hidden="true" className="text-xs text-slate-500">
          ▾
        </span>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="end"
          sideOffset={6}
          className={MENU_CLASSES}
        >
          <DropdownMenu.RadioGroup
            value={order}
            onValueChange={(value) => {
              if (isEngramsOrder(value)) {
                setOrder(value);
              }
            }}
          >
            {ENGRAMS_ORDERS.map((choice) => (
              <DropdownMenu.RadioItem
                key={choice}
                value={choice}
                className={ITEM_CLASSES}
              >
                <DropdownMenu.ItemIndicator>
                  <span aria-hidden="true">*</span>
                </DropdownMenu.ItemIndicator>
                {orderLabel(choice)}
              </DropdownMenu.RadioItem>
            ))}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
