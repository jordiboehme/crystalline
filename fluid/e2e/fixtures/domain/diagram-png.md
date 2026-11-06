---
type: guide
title: Diagram Picture Files
permalink: diagram-png
tags:
- smoke
status: current
recorded_at: 2026-02-04
timestamp: 2026-02-04T10:00:00+00:00
description: Diagrams for the journey that saves one as a picture file.
---

# Diagram Picture Files

A very wide flowchart, which the page shrinks to fit its column.

```mermaid
flowchart LR
    N0[wide_flow_step_number_0_of_the_long_chain] --> N1[wide_flow_step_number_1_of_the_long_chain] --> N2[wide_flow_step_number_2_of_the_long_chain] --> N3[wide_flow_step_number_3_of_the_long_chain] --> N4[wide_flow_step_number_4_of_the_long_chain] --> N5[wide_flow_step_number_5_of_the_long_chain] --> N6[wide_flow_step_number_6_of_the_long_chain] --> N7[wide_flow_step_number_7_of_the_long_chain] --> N8[wide_flow_step_number_8_of_the_long_chain] --> N9[wide_flow_step_number_9_of_the_long_chain] --> N10[wide_flow_step_number_10_of_the_long_chain] --> N11[wide_flow_step_number_11_of_the_long_chain] --> N12[wide_flow_step_number_12_of_the_long_chain] --> N13[wide_flow_step_number_13_of_the_long_chain] --> N14[wide_flow_step_number_14_of_the_long_chain] --> N15[wide_flow_step_number_15_of_the_long_chain] --> N16[wide_flow_step_number_16_of_the_long_chain] --> N17[wide_flow_step_number_17_of_the_long_chain] --> N18[wide_flow_step_number_18_of_the_long_chain] --> N19[wide_flow_step_number_19_of_the_long_chain] --> N20[wide_flow_step_number_20_of_the_long_chain] --> N21[wide_flow_step_number_21_of_the_long_chain] --> N22[wide_flow_step_number_22_of_the_long_chain] --> N23[wide_flow_step_number_23_of_the_long_chain] --> N24[wide_flow_step_number_24_of_the_long_chain] --> N25[wide_flow_step_number_25_of_the_long_chain] --> N26[wide_flow_step_number_26_of_the_long_chain] --> N27[wide_flow_step_number_27_of_the_long_chain] --> N28[wide_flow_step_number_28_of_the_long_chain] --> N29[wide_flow_step_number_29_of_the_long_chain] --> N30[wide_flow_step_number_30_of_the_long_chain] --> N31[wide_flow_step_number_31_of_the_long_chain] --> N32[wide_flow_step_number_32_of_the_long_chain] --> N33[wide_flow_step_number_33_of_the_long_chain] --> N34[wide_flow_step_number_34_of_the_long_chain] --> N35[wide_flow_step_number_35_of_the_long_chain] --> N36[wide_flow_step_number_36_of_the_long_chain] --> N37[wide_flow_step_number_37_of_the_long_chain] --> N38[wide_flow_step_number_38_of_the_long_chain] --> N39[wide_flow_step_number_39_of_the_long_chain]
```

The same kind of drawing with small text.

```mermaid
%%{init: {"themeVariables": {"fontSize": "9px"}}}%%
flowchart LR
    S0[small_text_first_step] --> S1[small_text_second_step] --> S2[small_text_third_step] --> S3[small_text_fourth_step]
```

A sequence diagram.

```mermaid
sequenceDiagram
    participant Harbor
    participant Pilot
    Harbor->>Pilot: Board the ship #35;4 near the buoy
    Note over Harbor,Pilot: Wait for the tide
    Pilot-->>Harbor: On board
```

A short flowchart.

```mermaid
flowchart TD
    A[Harbor &amp; pier] --> B{Tide high?}
    B -->|Yes| C[Enter]
    B -->|No| D[Wait]
```

A drawing with a picture from another place.

```mermaid
flowchart TD
    P@{ img: "https://pictures.example.invalid/ship.png", label: "A ship", pos: "t", w: 60, h: 60 }
```

A user journey with long words, whose labels the page wraps in their boxes.

```mermaid
journey
    title A long day
    section Getting on board in the early morning before the tide turns
      Walking all the way down the very long pier to the waiting ship: 5: Pilot
      Wait: 3: Pilot
```

A flowchart with thin lines.

```mermaid
flowchart LR
    T0[First] --> T1[Second]
    linkStyle default stroke-width:0.4px
```
