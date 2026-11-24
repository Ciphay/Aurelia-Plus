class Win64Abi:
    name = "win64"
    wraps_calls = False

    def thunk(self, ident, iface, slot, method, sig, argbytes=None, ret=None):
        log = f'shim_unimplemented("{iface}", {slot}, "{method or "?"}");'
        if ret == "struct8":
            return f'static uint64_t *{ident}(void *self, uint64_t *ret) {{ (void)self; {log} *ret = 0; return ret; }}\n'
        if ret == "call":
            return f'static uint64_t {ident}(void *self) {{ (void)self; {log} return shim_callresult_fail(); }}\n'
        return f'static uint64_t {ident}(void *self) {{ (void)self; {log} return 0; }}\n'
