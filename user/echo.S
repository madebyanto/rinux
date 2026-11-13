.intel_syntax noprefix

.section .text
.global _start
_start:
    mov r15, qword ptr [rsp]
    lea r14, [rsp + 8]

    lea rsi, [rip + msg_pid]
    mov edx, msg_pid_end - msg_pid
    call print_str
    mov eax, 39
    syscall
    call print_u64
    call print_nl

    lea rsi, [rip + msg_argv]
    mov edx, msg_argv_end - msg_argv
    call print_str
    call print_nl

    xor r13d, r13d
.argv_loop:
    cmp r13, r15
    jae .done
    lea rsi, [rip + msg_arg]
    mov edx, msg_arg_end - msg_arg
    call print_str
    mov rax, r13
    call print_u64
    lea rsi, [rip + msg_eq]
    mov edx, msg_eq_end - msg_eq
    call print_str
    mov rsi, qword ptr [r14 + r13 * 8]
    call strlen
    mov rdx, rax
    call print_str
    call print_nl
    inc r13
    jmp .argv_loop

.done:
    mov eax, 60
    mov edi, 7
    syscall

print_str:
    mov eax, 1
    mov edi, 1
    syscall
    ret

print_nl:
    lea rsi, [rip + newline]
    mov edx, 1
    call print_str
    ret

print_u64:
    sub rsp, 32
    lea rsi, [rsp + 32]
    mov rcx, 10
.digit:
    xor edx, edx
    div rcx
    add dl, 0x30
    dec rsi
    mov byte ptr [rsi], dl
    test rax, rax
    jne .digit
    lea rdx, [rsp + 32]
    sub rdx, rsi
    call print_str
    add rsp, 32
    ret

strlen:
    xor eax, eax
.len:
    cmp byte ptr [rsi + rax], 0
    je .len_done
    inc rax
    jmp .len
.len_done:
    ret

.section .rodata
msg_pid:
    .ascii "echo: pid "
msg_pid_end:
msg_argv:
    .ascii "echo: argv"
msg_argv_end:
msg_arg:
    .ascii "echo: argv["
msg_arg_end:
msg_eq:
    .ascii "]="
msg_eq_end:
newline:
    .byte 0x0A
