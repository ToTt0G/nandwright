// Draws a diagonal line from the top-left corner of the screen:
// pixel (k, k) for k = 0 .. 255.
    @SCREEN
    D=A
    @addr
    M=D           // word holding pixel (row 0, column 0)
    @mask
    M=1           // bit of column k within its word
    @256
    D=A
    @count
    M=D
(LOOP)
    @mask
    D=M
    @addr
    A=M
    M=D|M         // set the pixel
    @32
    D=A
    @addr
    M=D+M         // next row
    @mask
    D=M
    MD=D+M        // next column: shift the mask left
    @SAMEWORD
    D;JNE
    @mask         // the mask fell off the top bit:
    M=1           // first column of the next word
    @addr
    M=M+1
(SAMEWORD)
    @count
    MD=M-1
    @LOOP
    D;JGT
(END)
    @END
    0;JMP
