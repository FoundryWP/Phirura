// wp81d3d11.dll - does D3D11 actually work on Windows Phone 8.1, and can we reach the GPU?
//
// Win2D already proves the *device* has a working D3D11 stack - the app renders the chart through
// it. But Win2D is a black box: it says nothing about whether a hand-written D3D11 renderer, which
// is what a `miniquad` backend would be, can create a device, allocate a render target, draw into
// it, and read the result back.
//
// The first version of this probe answered "yes" - and reported `adapter="Software Adapter"` at
// feature level 9.3. That is WARP, the CPU rasteriser, which is useless for a rhythm game. Asking
// for `D3D_DRIVER_TYPE_HARDWARE` with a null adapter is the documented way to get the GPU on
// desktop Windows, so getting WARP back means that shortcut does not do what it says here.
//
// So this version enumerates the adapters through DXGI and reports every one of them, then creates
// the device on the first *hardware* adapter explicitly, with `D3D_DRIVER_TYPE_UNKNOWN` (the only
// legal driver type when an adapter is supplied). Only if that fails does it fall back to the
// driver-type shortcut and finally to WARP.
//
// Everything is offscreen: no swap chain, no XAML, no presentation. So a failure can be blamed on
// the device/context/texture path itself and nothing else.
//
// Build: cl /c, then link against d3d11.lib + dxgi.lib + dxguid.lib from
// `Windows Phone Kits\8.1\lib\ARM`. Called from C# as wp81_d3d11_probe(); the human-readable
// result is in wp81_d3d11_report().

#include <windows.h>
#include <d3d11.h>
#include <d3d11_1.h>
#include <dxgi.h>
#include <dxgi1_2.h>

// `memset` / `memcpy` are MSVC *intrinsics* - defining them is error C2169, not a workaround. The
// declaration is all that is needed: with /O2 they lower to inline code, so `ZeroMemory` (a macro
// for memset) costs nothing at run time and needs nothing from the CRT.
//
// This matters because `/NODEFAULTLIB` is forced here, not chosen: the only ARM CRT on this
// machine is VS11's desktop one (`VC\lib\arm\libcmt.lib`), whose startup objects import
// KERNEL32.dll, and WP8.1 has no KERNEL32.dll. Linking it yields a DLL that fails to load with
// ERROR_MOD_NOT_FOUND - the failure that sank the first native probe. The WP8.1 kit ships no CRT
// import lib of its own.
#include <string.h>

#define WP81_D3D11_API extern "C" __declspec(dllexport)

namespace {

const UINT kTexSize = 64;

// Distinct channels, none 0 or 255, so a read-back that is "close but not equal" is detectable
// rather than hidden by rounding. Expected bytes: 64, 128, 191, 255.
const FLOAT kClear[4] = { 0.25f, 0.50f, 0.75f, 1.00f };
const BYTE  kExpect[4] = { 64, 128, 191, 255 };

wchar_t g_report[1400];

// No CRT anywhere in this file on purpose: every helper below is hand-rolled so the DLL links
// with /NODEFAULTLIB, exactly like the Rust probe.

void Append(const wchar_t* text)
{
    size_t n = 0;
    while (g_report[n] != 0 && n < 1380) ++n;
    for (size_t i = 0; text[i] != 0 && n < 1398; ++i) g_report[n++] = text[i];
    g_report[n] = 0;
}

void AppendHex(unsigned long v)
{
    wchar_t rev[16];
    int i = 0;
    if (v == 0) rev[i++] = L'0';
    while (v > 0 && i < 15) {
        int d = (int)(v & 0xF);
        rev[i++] = (wchar_t)(d < 10 ? L'0' + d : L'A' + d - 10);
        v >>= 4;
    }
    wchar_t out[16];
    int k = 0;
    while (i > 0) out[k++] = rev[--i];
    out[k] = 0;
    Append(out);
}

// Decimal output without a division instruction.
//
// ARM has no guaranteed hardware divide, so MSVC lowers `x / 10` and `x % 10` into calls to
// `__rt_sdiv` / `__rt_sdiv64`, which live in the CRT - and there is no CRT here. Rather than link
// a division helper, this counts by repeated subtraction. Every value printed is a byte, an index
// or a feature level, so the loop runs a handful of times at most.
void AppendUInt(unsigned int v)
{
    if (v == 0) { Append(L"0"); return; }

    wchar_t rev[12];
    int i = 0;
    while (v > 0 && i < 11) {
        unsigned int rest = v;
        unsigned int tens = 0;
        while (rest >= 10) { rest -= 10; ++tens; }
        rev[i++] = (wchar_t)(L'0' + (int)rest);
        v = tens;
    }

    wchar_t out[12];
    int k = 0;
    while (i > 0) out[k++] = rev[--i];
    out[k] = 0;
    Append(out);
}

// The levels WP8.1 offers, newest first. 11_1 is included deliberately - see CreateOnAdapter.
const D3D_FEATURE_LEVEL kLevels[] = {
    D3D_FEATURE_LEVEL_11_1,
    D3D_FEATURE_LEVEL_11_0,
    D3D_FEATURE_LEVEL_10_1,
    D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_9_3,
    D3D_FEATURE_LEVEL_9_2,
    D3D_FEATURE_LEVEL_9_1,
};
const UINT kLevelCount = ARRAYSIZE(kLevels);

// Create a device on a given adapter.
//
// D3D_FEATURE_LEVEL_11_1 is a documented trap: if the installed runtime predates 11.1, passing it
// at all makes the call fail with E_INVALIDARG rather than quietly falling back. WP8.1 shipped with
// the 11.0 runtime, so this is exactly the situation the docs warn about - and handling it is the
// difference between a working renderer and one that refuses to start on a phone that supports
// D3D11 perfectly well.
HRESULT CreateOnAdapter(IDXGIAdapter* adapter, ID3D11Device** dev,
                        D3D_FEATURE_LEVEL* lvl, ID3D11DeviceContext** ctx)
{
    HRESULT hr = D3D11CreateDevice(
        adapter, D3D_DRIVER_TYPE_UNKNOWN, nullptr, 0,
        kLevels, kLevelCount, D3D11_SDK_VERSION, dev, lvl, ctx);

    if (hr == E_INVALIDARG) {
        hr = D3D11CreateDevice(
            adapter, D3D_DRIVER_TYPE_UNKNOWN, nullptr, 0,
            kLevels + 1, kLevelCount - 1, D3D11_SDK_VERSION, dev, lvl, ctx);
    }
    return hr;
}

}  // namespace

WP81_D3D11_API long __stdcall wp81_d3d11_probe()
{
    g_report[0] = 0;

    ID3D11Device*        device  = nullptr;
    ID3D11DeviceContext* context = nullptr;
    D3D_FEATURE_LEVEL    level   = (D3D_FEATURE_LEVEL)0;
    HRESULT hr;

    // ---------------------------------------------------------------------
    // 1. Enumerate every adapter. This is the part the first probe was missing: it reported the
    //    adapter it happened to get but never asked what was available, so "Software Adapter"
    //    looked like a property of the phone rather than of the call.
    // ---------------------------------------------------------------------
    IDXGIAdapter1* hardwareAdapter = nullptr;

    IDXGIFactory1* factory = nullptr;
    hr = CreateDXGIFactory1(__uuidof(IDXGIFactory1), (void**)&factory);
    Append(L"1 factory hr=0x");
    AppendHex((unsigned long)hr);

    if (SUCCEEDED(hr) && factory) {
        UINT i = 0;
        for (;;) {
            IDXGIAdapter1* ad = nullptr;
            if (factory->EnumAdapters1(i, &ad) != S_OK || ad == nullptr) break;

            DXGI_ADAPTER_DESC1 d;
            if (SUCCEEDED(ad->GetDesc1(&d))) {
                Append(L" [");
                AppendUInt(i);
                Append(L" \"");
                Append(d.Description);
                Append(L"\" flags=0x");
                AppendHex(d.Flags);
                // DXGI_ADAPTER_FLAG_SOFTWARE (2) marks WARP. The first adapter without it is the
                // real GPU.
                if ((d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) == 0 && hardwareAdapter == nullptr) {
                    hardwareAdapter = ad;
                    ad->AddRef();
                }
                Append(L"]");
            }
            ad->Release();
            ++i;
        }
        Append(L" count=");
        AppendUInt(i);
        factory->Release();
    }

    // ---------------------------------------------------------------------
    // 2. Create the device, preferring a real hardware adapter.
    // ---------------------------------------------------------------------
    Append(L" | 2");

    if (hardwareAdapter != nullptr) {
        hr = CreateOnAdapter(hardwareAdapter, &device, &level, &context);
        Append(L" viaAdapter hr=0x");
        AppendHex((unsigned long)hr);
    } else {
        hr = E_FAIL;
        Append(L" noHardwareAdapter");
    }

    if (FAILED(hr) || device == nullptr) {
        // Fall back to the driver-type shortcut, which is what the first probe used and which
        // returned WARP. Recorded explicitly, because "we ended up on WARP" and "we asked for
        // WARP" are very different findings.
        Append(L" -> fallback DRIVER_TYPE_HARDWARE");
        hr = D3D11CreateDevice(
            nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0,
            kLevels, kLevelCount, D3D11_SDK_VERSION, &device, &level, &context);
        if (hr == E_INVALIDARG) {
            hr = D3D11CreateDevice(
                nullptr, D3D_DRIVER_TYPE_HARDWARE, nullptr, 0,
                kLevels + 1, kLevelCount - 1, D3D11_SDK_VERSION, &device, &level, &context);
        }
        Append(L" hr=0x");
        AppendHex((unsigned long)hr);
    }

    if (hardwareAdapter != nullptr) hardwareAdapter->Release();

    if (FAILED(hr) || device == nullptr || context == nullptr) {
        Append(L" FAILED");
        return 1;
    }
    Append(L" level=0x");
    AppendHex((unsigned long)level);

    // ---------------------------------------------------------------------
    // 3. Render target: a 64x64 RGBA texture, cleared to a known colour.
    // ---------------------------------------------------------------------
    D3D11_TEXTURE2D_DESC td;
    ZeroMemory(&td, sizeof(td));
    td.Width            = kTexSize;
    td.Height           = kTexSize;
    td.MipLevels        = 1;
    td.ArraySize        = 1;
    td.Format           = DXGI_FORMAT_R8G8B8A8_UNORM;
    td.SampleDesc.Count = 1;
    td.Usage            = D3D11_USAGE_DEFAULT;
    td.BindFlags        = D3D11_BIND_RENDER_TARGET;

    ID3D11Texture2D* target = nullptr;
    hr = device->CreateTexture2D(&td, nullptr, &target);
    Append(L" | 3 texture hr=0x");
    AppendHex((unsigned long)hr);
    if (FAILED(hr) || target == nullptr) { Append(L" FAILED"); return 3; }

    ID3D11RenderTargetView* rtv = nullptr;
    hr = device->CreateRenderTargetView(target, nullptr, &rtv);
    Append(L" rtv hr=0x");
    AppendHex((unsigned long)hr);
    if (FAILED(hr) || rtv == nullptr) { Append(L" FAILED"); return 4; }

    // ---------------------------------------------------------------------
    // 4. Clear. This is the first call that actually executes GPU work.
    // ---------------------------------------------------------------------
    context->ClearRenderTargetView(rtv, kClear);
    context->Flush();
    Append(L" | 4 cleared");

    // ---------------------------------------------------------------------
    // 5. Read it back. Needs a STAGING copy, because a DEFAULT-usage texture cannot be mapped.
    //    This is the same path any renderer needs for screenshots or for reading a computed
    //    result, so it is worth proving rather than assuming.
    // ---------------------------------------------------------------------
    D3D11_TEXTURE2D_DESC sd = td;
    sd.Usage          = D3D11_USAGE_STAGING;
    sd.BindFlags      = 0;
    sd.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    sd.MiscFlags      = 0;

    ID3D11Texture2D* staging = nullptr;
    hr = device->CreateTexture2D(&sd, nullptr, &staging);
    Append(L" | 5 staging hr=0x");
    AppendHex((unsigned long)hr);
    if (FAILED(hr) || staging == nullptr) { Append(L" FAILED"); return 5; }

    context->CopyResource(staging, target);

    D3D11_MAPPED_SUBRESOURCE mapped;
    ZeroMemory(&mapped, sizeof(mapped));
    hr = context->Map(staging, 0, D3D11_MAP_READ, 0, &mapped);
    Append(L" map hr=0x");
    AppendHex((unsigned long)hr);
    if (FAILED(hr) || mapped.pData == nullptr) { Append(L" FAILED"); return 6; }

    const BYTE* row = (const BYTE*)mapped.pData;
    BYTE got[4] = { row[0], row[1], row[2], row[3] };
    context->Unmap(staging, 0);

    Append(L" pixel=");
    AppendUInt(got[0]); Append(L","); AppendUInt(got[1]); Append(L",");
    AppendUInt(got[2]); Append(L","); AppendUInt(got[3]);
    Append(L" want=");
    AppendUInt(kExpect[0]); Append(L","); AppendUInt(kExpect[1]); Append(L",");
    AppendUInt(kExpect[2]); Append(L","); AppendUInt(kExpect[3]);

    // A +/-1 tolerance: the clear colour is converted through UNORM and back, and drivers are
    // allowed to round either way.
    bool ok = true;
    for (int i = 0; i < 4; ++i) {
        int d = (int)got[i] - (int)kExpect[i];
        if (d < -1 || d > 1) ok = false;
    }

    staging->Release();
    rtv->Release();
    target->Release();
    context->Release();
    device->Release();

    if (!ok) { Append(L" MISMATCH"); return 7; }

    Append(L" | ALL OK");
    return 0;
}

WP81_D3D11_API const wchar_t* __stdcall wp81_d3d11_report()
{
    return g_report;
}
