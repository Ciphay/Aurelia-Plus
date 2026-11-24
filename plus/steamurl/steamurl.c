#include <windows.h>
#include <wchar.h>

typedef int (*keyboard_fn)(int);
typedef int (*open_fn)(void);

int WINAPI wWinMain(HINSTANCE inst, HINSTANCE prev, LPWSTR cmd, int show) {
    (void)inst; (void)prev; (void)cmd; (void)show;
    const wchar_t *line = GetCommandLineW();
    BOOL want_open = wcsstr(line, L"steam://open/keyboard") != NULL;
    BOOL want_close = wcsstr(line, L"steam://close/keyboard") != NULL;
    if (!want_open && !want_close) return 0;

    HANDLE closing = CreateEventW(NULL, TRUE, FALSE, L"Aurelia_OSK_Close");
    if (want_close) {
        if (closing) SetEvent(closing);
        return 0;
    }
    HANDLE owner = CreateMutexW(NULL, FALSE, L"Aurelia_OSK_Open");
    if (!owner || GetLastError() == ERROR_ALREADY_EXISTS) return 0;
    if (closing) ResetEvent(closing);

    wchar_t path[MAX_PATH];
    DWORD n = GetModuleFileNameW(NULL, path, MAX_PATH);
    while (n > 0 && path[n - 1] != L'\\' && path[n - 1] != L'/') n--;
    wcscpy(path + n, L"steamclient64.dll");
    HMODULE shim = LoadLibraryW(path);
    if (!shim) return 2;
    keyboard_fn keyboard = (keyboard_fn)GetProcAddress(shim, "Aurelia_Keyboard");
    open_fn is_open = (open_fn)GetProcAddress(shim, "Aurelia_KeyboardOpen");
    if (!keyboard || !keyboard(1)) return 3;
    for (;;) {
        if (closing && WaitForSingleObject(closing, 300) == WAIT_OBJECT_0) break;
        if (!closing) Sleep(300);
        if (is_open && !is_open()) return 0;
    }
    keyboard(0);
    return 0;
}
