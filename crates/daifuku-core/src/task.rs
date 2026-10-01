//! The scheduled task that starts the daemon, as Task Scheduler XML.
//!
//! A task rather than a Run key, because a Run key starts programs without
//! elevation and the daemon needs it: `HighestAvailable` starts it elevated
//! at logon with no prompt. The task runs only while the user is signed in
//! (`InteractiveToken`): the daemon draws on that user's desktop, and a task
//! that runs whether or not anyone is signed in gets no desktop at all.
//!
//! It runs at normal priority. Task Scheduler's default is below normal, and
//! Windows passes that on to every program the daemon starts: the terminals
//! of an administrator fleet, and every agent and build run in them.

/// Where the task lives in Task Scheduler.
pub const TASK_NAME: &str = r"\Daifuku\Daemon";

/// The task XML for `exe`, run for the user with SID `user_sid`.
#[must_use]
pub fn xml(exe: &str, user_sid: &str) -> String {
    let exe = escape(exe);
    let sid = escape(user_sid);
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Daifuku: fleets of agent terminals and their status borders.</Description>
    <URI>{TASK_NAME}</URI>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{sid}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{sid}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
    </Exec>
  </Actions>
</Task>
"#
    )
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_elevated_at_logon_for_that_user_only_and_never_times_out() {
        let x = xml(
            r"C:\Program Files\Daifuku\daifukud.exe",
            "S-1-5-21-1-2-3-1001",
        );
        assert!(x.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(x.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(x.contains("<LogonTrigger>") && x.matches("S-1-5-21-1-2-3-1001").count() == 2);
        assert!(x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(x.contains(r"<Command>C:\Program Files\Daifuku\daifukud.exe</Command>"));
    }

    /// 4 to 6 are the normal class; 7, the default, is below normal.
    #[test]
    fn runs_at_normal_priority() {
        let x = xml(r"C:\Program Files\Daifuku\daifukud.exe", "S");
        assert!(x.contains("<Priority>4</Priority>"));
    }

    #[test]
    fn a_path_cannot_break_out_of_the_xml() {
        let x = xml(r#"C:\a&b<c>"d'.exe"#, "S");
        assert!(x.contains(r"<Command>C:\a&amp;b&lt;c&gt;&quot;d&apos;.exe</Command>"));
    }
}
