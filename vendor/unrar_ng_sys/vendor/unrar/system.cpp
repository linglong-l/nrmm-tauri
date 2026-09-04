#include "rar.hpp"
#if defined(__GNUC__) && !defined(_APPLE)
// For manual CPUID replacement of __builtin_cpu_supports (see GetSSEVersion below).
// __builtin_cpu_supports causes clang/zig cc to emit R_X86_64_PC32 against
// __cpu_model, which lld rejects when linking the crate's objects into a cdylib.
#include <cpuid.h>
#endif

static int SleepTime=0;

void InitSystemOptions(int SleepTime)
{
  ::SleepTime=SleepTime;
}


#if !defined(SFX_MODULE)
void SetPriority(int Priority)
{
#ifdef _WIN_ALL
  uint PriorityClass;
  int PriorityLevel;
  if (Priority<1 || Priority>15)
    return;

  if (Priority==1)
  {
    PriorityClass=IDLE_PRIORITY_CLASS;
    PriorityLevel=THREAD_PRIORITY_IDLE;

//  Background mode for Vista, can be slow for many small files.
//    if (WinNT()>=WNT_VISTA)
//      SetPriorityClass(GetCurrentProcess(),PROCESS_MODE_BACKGROUND_BEGIN);
  }
  else
    if (Priority<7)
    {
      PriorityClass=IDLE_PRIORITY_CLASS;
      PriorityLevel=Priority-4;
    }
    else
      if (Priority==7)
      {
        PriorityClass=BELOW_NORMAL_PRIORITY_CLASS;
        PriorityLevel=THREAD_PRIORITY_ABOVE_NORMAL;
      }
      else
        if (Priority<10)
        {
          PriorityClass=NORMAL_PRIORITY_CLASS;
          PriorityLevel=Priority-7;
        }
        else
          if (Priority==10)
          {
            PriorityClass=ABOVE_NORMAL_PRIORITY_CLASS;
            PriorityLevel=THREAD_PRIORITY_NORMAL;
          }
          else
          {
            PriorityClass=HIGH_PRIORITY_CLASS;
            PriorityLevel=Priority-13;
          }
  SetPriorityClass(GetCurrentProcess(),PriorityClass);
  SetThreadPriority(GetCurrentThread(),PriorityLevel);

#ifdef RAR_SMP
  ThreadPool::SetPriority(PriorityLevel);
#endif

#endif
}
#endif


// Monotonic clock. Like clock(), returns time passed in CLOCKS_PER_SEC items.
// In Android 5+ and Unix usual clock() returns time spent by all threads
// together, so we cannot use it to measure time intervals anymore.
clock_t MonoClock()
{
  return clock();
}



void Wait()
{
  if (ErrHandler.UserBreak)
    ErrHandler.Exit(RARX_USERBREAK);
#if defined(_WIN_ALL) && !defined(SFX_MODULE)
  if (SleepTime!=0)
  {
    static clock_t LastTime=MonoClock();
    if (MonoClock()-LastTime>10*CLOCKS_PER_SEC/1000)
    {
      Sleep(SleepTime);
      LastTime=MonoClock();
    }
  }
#endif
#if defined(_WIN_ALL)
  // Reset system sleep timer to prevent system going sleep.
  SetThreadExecutionState(ES_SYSTEM_REQUIRED);
#endif
}




#ifdef _WIN_ALL
bool SetPrivilege(LPCTSTR PrivName)
{
  bool Success=false;

  HANDLE hToken;
  if (OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES, &hToken))
  {
    TOKEN_PRIVILEGES tp;
    tp.PrivilegeCount = 1;
    tp.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;

    if (LookupPrivilegeValue(NULL,PrivName,&tp.Privileges[0].Luid) &&
        AdjustTokenPrivileges(hToken, FALSE, &tp, 0, NULL, NULL) &&
        GetLastError() == ERROR_SUCCESS)
      Success=true;

    CloseHandle(hToken);
  }

  return Success;
}
#endif


#if defined(_WIN_ALL) && !defined(SFX_MODULE)
void Shutdown(POWER_MODE Mode)
{
  SetPrivilege(SE_SHUTDOWN_NAME);
  if (Mode==POWERMODE_OFF)
    ExitWindowsEx(EWX_SHUTDOWN|EWX_FORCE,SHTDN_REASON_FLAG_PLANNED);
  if (Mode==POWERMODE_SLEEP)
    SetSuspendState(FALSE,FALSE,FALSE);
  if (Mode==POWERMODE_HIBERNATE)
    SetSuspendState(TRUE,FALSE,FALSE);
  if (Mode==POWERMODE_RESTART)
    ExitWindowsEx(EWX_REBOOT|EWX_FORCE,SHTDN_REASON_FLAG_PLANNED);
}


bool ShutdownCheckAnother(bool Open)
{
  const wchar *EventName=L"rar -ioff";
  static HANDLE hEvent=NULL;
  bool Result=false; // Return false if no other RAR -ioff are running.
  if (Open) // Create or open the event.
    hEvent=CreateEvent(NULL,FALSE,FALSE,EventName);
  else
  {
    if (hEvent!=NULL)
      CloseHandle(hEvent); // Close our event.
    // Check if other copies still own the event. While race conditions
    // are possible, they are improbable and their harm is minimal.
    hEvent=CreateEvent(NULL,FALSE,FALSE,EventName);
    Result=GetLastError()==ERROR_ALREADY_EXISTS;
    if (hEvent!=NULL)
      CloseHandle(hEvent);
  }
  return Result;
}
#endif





#if defined(_WIN_ALL)
// Load library from Windows System32 folder. Use this function to prevent
// loading a malicious code from current folder or same folder as exe.
HMODULE WINAPI LoadSysLibrary(const wchar *Name)
{
  std::vector<wchar> SysDir(MAX_PATH);
  if (GetSystemDirectory(SysDir.data(),(UINT)SysDir.size())==0)
    return nullptr;
  std::wstring FullName;
  MakeName(SysDir.data(),Name,FullName);
  return LoadLibrary(FullName.c_str());
}


bool IsUserAdmin()
{
  SID_IDENTIFIER_AUTHORITY NtAuthority = SECURITY_NT_AUTHORITY;
  PSID AdministratorsGroup; 
  BOOL b = AllocateAndInitializeSid(&NtAuthority,2,SECURITY_BUILTIN_DOMAIN_RID,
           DOMAIN_ALIAS_RID_ADMINS, 0, 0, 0, 0, 0, 0, &AdministratorsGroup); 
  if (b) 
  {
    if (!CheckTokenMembership( NULL, AdministratorsGroup, &b)) 
      b = FALSE;
    FreeSid(AdministratorsGroup); 
  }
  return b!=FALSE;
}

#endif


#ifdef USE_SSE
SSE_VERSION _SSE_Version=GetSSEVersion();

SSE_VERSION GetSSEVersion()
{
#ifdef _MSC_VER
  int CPUInfo[4];
  __cpuid(CPUInfo, 0);

  // Maximum supported cpuid function.
  uint MaxSupported=CPUInfo[0];

  if (MaxSupported>=7)
  {
    __cpuid(CPUInfo, 7);
    if ((CPUInfo[1] & 0x20)!=0)
      return SSE_AVX2;
  }
  if (MaxSupported>=1)
  {
    __cpuid(CPUInfo, 1);
    if ((CPUInfo[2] & 0x80000)!=0)
      return SSE_SSE41;
    if ((CPUInfo[2] & 0x200)!=0)
      return SSE_SSSE3;
    if ((CPUInfo[3] & 0x4000000)!=0)
      return SSE_SSE2;
    if ((CPUInfo[3] & 0x2000000)!=0)
      return SSE_SSE;
  }
#elif defined(__GNUC__) && !defined(_APPLE)
  // Manual CPUID feature detection (avoids __builtin_cpu_supports which forces
  // clang/zig cc to emit non-PIC R_X86_64_PC32 against __cpu_model, breaking
  // cdylib linking via lld). Leaf 1 -> ECX/EDX for SSE family; leaf 7 sub 0
  // -> EBX bit 5 for AVX2. Uses inline asm for leaf 7 because clang's
  // __cpuid_count builtin conflicts with <cpuid.h>'s macro definition.
  unsigned int __leax=0, __lebx=0, __lecx=0, __ledx=0;
  if (__get_cpuid(1, &__leax, &__lebx, &__lecx, &__ledx))
  {
    if (__lecx & (1u<<19)) return SSE_SSE41;
    if (__lecx & (1u<<9))  return SSE_SSSE3;
    if (__ledx & (1u<<26)) return SSE_SSE2;
    if (__ledx & (1u<<25)) return SSE_SSE;
  }
  unsigned int __maxleaf=0;
  __get_cpuid(0, &__maxleaf, &__lebx, &__lecx, &__ledx);
  if (__maxleaf >= 7)
  {
    __asm__ __volatile__(
      "cpuid"
      : "=a"(__leax), "=b"(__lebx), "=c"(__lecx), "=d"(__ledx)
      : "a"(7u), "c"(0u)
    );
    if (__lebx & (1u<<5)) return SSE_AVX2;
  }
#endif
  return SSE_NONE;
}
#endif
