## Fill in `PARTS:`

```hdl
CHIP Majority {
    IN a, b, c;
    OUT out;

    PARTS:
    And(a=a, b=b, out=ab);
    ...
}
```

While you type:

- **Completion** lists the chips you can use, then their pins.
- **Hover** over a chip name to see its inputs and outputs.
- **Errors** are underlined where they are: an unknown chip, a pin that does
  not exist, buses of different widths, a wire that nothing drives.
- **Format Document** tidies indentation and spacing. It never moves code or
  comments to another line.

A chip whose `PARTS:` is still empty is not an error. Its test will fail,
because its outputs stay 0, exactly as in the official Hardware Simulator.
