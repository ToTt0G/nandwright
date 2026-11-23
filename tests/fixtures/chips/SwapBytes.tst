load SwapBytes.hdl,
output-file SwapBytes.out,
compare-to SwapBytes.cmp,
output-list in%B1.16.1 out%B1.16.1 out%D1.6.1 out%X1.4.1;

set in %B0000000011111111, eval, output;
set in %B1111111100000000, eval, output;
set in %X12AB, eval, output;
set in 258, eval, output;
set in -1, eval, output;
set in 0, eval, output;
