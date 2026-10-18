//! Helpers the unit tests share.

use windows::Win32::Foundation::HANDLE;

/// Runs `f` on this thread with the rights an ordinary process of this
/// user has: Administrators for deny only, no privileges beyond the one
/// every process has, and medium integrity. An elevated test run, as on
/// CI, then meets the access checks an ordinary process meets.
pub fn as_ordinary_user<T>(f: impl FnOnce() -> T) -> T {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Security::{
        CreateRestrictedToken, CreateWellKnownSid, DISABLE_MAX_PRIVILEGE, DuplicateTokenEx,
        GetLengthSid, PSID, RevertToSelf, SECURITY_MAX_SID_SIZE, SID_AND_ATTRIBUTES,
        SecurityImpersonation, SetTokenInformation, TOKEN_ADJUST_DEFAULT, TOKEN_DUPLICATE,
        TOKEN_IMPERSONATE, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenImpersonation,
        TokenIntegrityLevel, WELL_KNOWN_SID_TYPE, WinBuiltinAdministratorsSid, WinMediumLabelSid,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken, SetThreadToken};

    /// Marks a SID as an integrity level, `SE_GROUP_INTEGRITY`.
    const SE_GROUP_INTEGRITY: u32 = 0x20;

    /// Goes back to the process's own rights, also when `f` panics.
    struct Revert(HANDLE);
    impl Drop for Revert {
        fn drop(&mut self) {
            // SAFETY: undoes the SetThreadToken below and closes its token.
            unsafe {
                let _ = RevertToSelf();
                let _ = CloseHandle(self.0);
            }
        }
    }

    /// A well-known SID, in a buffer of its own.
    fn sid(kind: WELL_KNOWN_SID_TYPE) -> Vec<u8> {
        let mut buf = vec![0u8; SECURITY_MAX_SID_SIZE as usize];
        let mut len = SECURITY_MAX_SID_SIZE;
        // SAFETY: buf holds len bytes.
        unsafe {
            CreateWellKnownSid(
                kind,
                None,
                Some(PSID(buf.as_mut_ptr().cast())),
                &raw mut len,
            )
        }
        .unwrap();
        buf
    }

    let (mut admins, mut medium) = (sid(WinBuiltinAdministratorsSid), sid(WinMediumLabelSid));
    // SAFETY: every token is closed on every path, and the SID buffers
    // outlive the calls that read them.
    unsafe {
        let mut own = HANDLE::default();
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE | TOKEN_QUERY,
            &raw mut own,
        )
        .unwrap();
        let disable = [SID_AND_ATTRIBUTES {
            Sid: PSID(admins.as_mut_ptr().cast()),
            Attributes: 0,
        }];
        let mut restricted = HANDLE::default();
        let made = CreateRestrictedToken(
            own,
            DISABLE_MAX_PRIVILEGE,
            Some(&disable),
            None,
            None,
            &raw mut restricted,
        );
        let _ = CloseHandle(own);
        made.unwrap();
        let mut ordinary = HANDLE::default();
        let duplicated = DuplicateTokenEx(
            restricted,
            TOKEN_IMPERSONATE | TOKEN_QUERY | TOKEN_ADJUST_DEFAULT,
            None,
            SecurityImpersonation,
            TokenImpersonation,
            &raw mut ordinary,
        );
        let _ = CloseHandle(restricted);
        duplicated.unwrap();
        let revert = Revert(ordinary);
        let level = PSID(medium.as_mut_ptr().cast());
        let label = TOKEN_MANDATORY_LABEL {
            Label: SID_AND_ATTRIBUTES {
                Sid: level,
                Attributes: SE_GROUP_INTEGRITY,
            },
        };
        let size = u32::try_from(size_of::<TOKEN_MANDATORY_LABEL>()).unwrap() + GetLengthSid(level);
        SetTokenInformation(
            ordinary,
            TokenIntegrityLevel,
            (&raw const label).cast(),
            size,
        )
        .unwrap();
        SetThreadToken(None, Some(ordinary)).unwrap();
        let out = f();
        drop(revert);
        out
    }
}
