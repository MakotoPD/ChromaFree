#pragma once

void LogLine(const wchar_t* format, ...);
std::wstring Win32ErrorText(DWORD error);
std::wstring ProcessDiagnostics();
int64_t QpcNow();
double QpcToMilliseconds(int64_t ticks);
