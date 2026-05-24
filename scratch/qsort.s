.intel_syntax noprefix
.global qsort
.text

qsort:
    # Save callee-saved registers
    push rbx
    push rbp
    push r12
    push r13
    push r14
    push r15

    # Store arguments in callee-saved registers
    mov r12, rdi   # r12 = base
    mov r13, rsi   # r13 = nmemb
    mov r14, rdx   # r14 = size
    mov r15, rcx   # r15 = compar

    cmp r13, 2
    jb .Ldone      # if nmemb < 2, return

    mov rbx, 1     # rbx = i (outer loop)

.Louter_loop:
    cmp rbx, r13
    jae .Ldone     # if i >= nmemb, done

    mov rbp, rbx   # rbp = j (inner loop)

.Linner_loop:
    test rbp, rbp
    jz .Linner_done # if j == 0, break

    # Compute p1 = base + j * size
    mov rax, rbp
    imul rax, r14
    add rax, r12
    mov rdi, rax   # rdi = p1 (arg0 for compar)

    # Compute p2 = base + (j - 1) * size
    sub rax, r14
    mov rsi, rax   # rsi = p2 (arg1 for compar)

    # Call comparator
    call r15       # eax = compar(p1, p2)

    # Check result
    test eax, eax
    jge .Linner_done # if compar(p1, p2) >= 0, break inner loop

    # Recompute p1 and p2 (since compar could clobber rdi/rsi)
    mov rdi, rbp
    imul rdi, r14
    add rdi, r12   # rdi = p1
    mov rsi, rdi
    sub rsi, r14   # rsi = p2

    # Swap elements of size r14
    xor rcx, rcx   # rcx = k = 0
.Lswap_loop:
    cmp rcx, r14
    jae .Lswap_done
    mov al, [rdi + rcx]
    mov dl, [rsi + rcx]
    mov [rsi + rcx], al
    mov [rdi + rcx], dl
    inc rcx
    jmp .Lswap_loop

.Lswap_done:
    dec rbp        # j--
    jmp .Linner_loop

.Linner_done:
    inc rbx        # i++
    jmp .Louter_loop

.Ldone:
    # Restore callee-saved registers
    pop r15
    pop r14
    pop r13
    pop r12
    pop rbp
    pop rbx
    ret
