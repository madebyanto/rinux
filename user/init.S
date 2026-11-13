.intel_syntax noprefix

.section .text
.global _start
_start:
    lea rsi, [rip + msg_pid]
    mov edx, msg_pid_end - msg_pid
    call print_str
    mov eax, 39
    syscall
    call print_u64
    call print_nl

    mov eax, 57
    syscall
    test rax, rax
    jz .child
    mov r12, rax

    lea rsi, [rip + msg_fork]
    mov edx, msg_fork_end - msg_fork
    call print_str
    mov rax, r12
    call print_u64
    call print_nl

    lea rsi, [rip + msg_wait]
    mov edx, msg_wait_end - msg_wait
    call print_str
    mov rax, r12
    call print_u64
    call print_nl

    mov edi, r12d
    lea rsi, [rip + status]
    xor edx, edx
    mov eax, 61
    syscall
    mov eax, dword ptr [rip + status]
    shr eax, 8
    and eax, 0xFF
    mov r13, rax

    lea rsi, [rip + msg_exit]
    mov edx, msg_exit_end - msg_exit
    call print_str
    mov rax, r13
    call print_u64
    call print_nl

    mov eax, 60
    xor edi, edi
    syscall

.child:
    lea rdi, [rip + echo_path]
    lea rsi, [rip + echo_argv]
    lea rdx, [rip + envp]
    mov eax, 59
    syscall
    lea rsi, [rip + msg_fail]
    mov edx, msg_fail_end - msg_fail
    call print_str
    mov eax, 60
    mov edi, 127
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

.section .rodata
msg_pid:
    .ascii "init: pid "
msg_pid_end:
msg_fork:
    .ascii "init: fork -> child "
msg_fork_end:
msg_wait:
    .ascii "init: waiting for child "
msg_wait_end:
msg_exit:
    .ascii "init: child exited with code "
msg_exit_end:
msg_fail:
    .ascii "init: execve failed\n"
msg_fail_end:
newline:
    .byte 0x0A
echo_path:
    .asciz "/bin/echo.elf"
echo_arg1:
    .asciz "hello"
echo_arg2:
    .asciz "execve"

.section .data
.align 8
echo_argv:
    .quad echo_path, echo_arg1, echo_arg2, 0
envp:
    .quad 0

.section .bss
.align 8
status:
    .zero 8
