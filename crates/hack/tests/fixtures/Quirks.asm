// Syntax the official assembler accepts although the book does not show it.
// Every line here assembles identically with tools/Assembler.sh.
	@ 1 2	// spaces and tabs are removed anywhere: this is @12
D = M ; JGT
=M          // empty destination
D;          // empty jump
AMD=A&D     // operand order swapped (alias of D&A)
M=M+D
D=A|D
D=M&D
@-1         // negative constant: assembles to 1111111111111111
@-32768
@+7
@32768      // out of range, so it is a variable
@1st        // symbols may start with a digit
@a+b
@x.y$z:w_1
(SP)        // a label may redefine a predefined symbol
@SP
(DUP)
@DUP
0;JMP
(DUP)       // the last definition wins
@ R 1 5
@Δx
