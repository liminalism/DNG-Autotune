@echo off
REM Agent build/test loop for raw-autotune. Double-click this file; it writes
REM .agent\check.log and leaves the window open at the end.
REM
REM Everything runs in RELEASE. That is not a preference, it is the only way the
REM loop is usable here: the test suite decodes real RAW files, and a debug-mode
REM demosaic on a 24 MP frame is minutes rather than seconds. Sharing the release
REM profile between `check`, `clippy` and `test` also means one set of artifacts
REM instead of three.
REM
REM   check.bat          -> fmt + check --all-targets + clippy   (default)
REM   check.bat test     -> the above, plus the full test suite
REM   check.bat focus X  -> the above, plus `cargo test --release X`
REM   check.bat sky      -> build, then render the frames that showed the blown
REM                         -sky cast into raw-autotune-output\sky-check for
REM                         eyeball grading.
REM   check.bat sweep    -> build, then render the sky frames plus controls at a
REM                         range of --highlight-contrast and --local-tone
REM                         settings and print the graded table. This is the run
REM                         that decides the sky taste question; the eye still
REM                         gets the last word, but the numbers say where to look.

setlocal
set ROOT=%~dp0
set LOG=%ROOT%.agent\check.log
if not exist "%ROOT%.agent" mkdir "%ROOT%.agent"

echo ==== check.bat started %DATE% %TIME% ====> "%LOG%"

echo.
echo [1/3] cargo fmt
echo ---- cargo fmt ---->> "%LOG%"
pushd "%ROOT%"
cargo fmt >> "%LOG%" 2>&1
echo exit=%ERRORLEVEL% >> "%LOG%"
popd

REM --all-targets, because a plain `cargo check` never looks at test or example
REM targets: an orphaned test file or a stale probe-* example compiles for nobody
REM until `cargo test` runs, and then it fails at the worst moment.
echo [2/3] cargo check --all-targets --release
echo ---- cargo check --all-targets --release ---->> "%LOG%"
pushd "%ROOT%"
cargo check --all-targets --release --message-format=short >> "%LOG%" 2>&1
echo exit=%ERRORLEVEL% >> "%LOG%"
popd

echo [3/3] cargo clippy --lib --bins --release
echo ---- cargo clippy --lib --bins --release ---->> "%LOG%"
pushd "%ROOT%"
cargo clippy --lib --bins --release --message-format=short >> "%LOG%" 2>&1
echo exit=%ERRORLEVEL% >> "%LOG%"
popd

if /I "%~1"=="test" goto runtests
if /I "%~1"=="focus" goto runfocus
if /I "%~1"=="sky" goto runsky
if /I "%~1"=="sweep" goto runsweep
goto done

:runtests
echo [4/4] cargo test --release
echo ---- cargo test --release ---->> "%LOG%"
pushd "%ROOT%"
cargo test --release >> "%LOG%" 2>&1
echo exit=%ERRORLEVEL% >> "%LOG%"
popd
goto done

:runfocus
echo [4/4] cargo test --release %~2
echo ---- cargo test --release %~2 ---->> "%LOG%"
pushd "%ROOT%"
cargo test --release %~2 >> "%LOG%" 2>&1
echo exit=%ERRORLEVEL% >> "%LOG%"
popd
goto done

:runsky
REM The five frames from raw\raw_3rd_batch that carried the lavender-magenta sky.
REM Rendering only these keeps the visual loop to seconds; the full-corpus
REM regression sweep (docs\STATUS.md) is a separate, deliberate run.
set SKY=%ROOT%raw-autotune-output\sky-check
if not exist "%SKY%" mkdir "%SKY%"
echo [4/5] cargo build --release
echo ---- cargo build --release ---->> "%LOG%"
pushd "%ROOT%"
cargo build --release --message-format=short >> "%LOG%" 2>&1
echo exit=%ERRORLEVEL% >> "%LOG%"

echo [5/5] rendering the blown-sky frames to raw-autotune-output\sky-check
echo ---- sky render ---->> "%LOG%"
REM One invocation, one summary: the summary entries carry
REM color.highlight_reconstruction for every frame, so the numbers and the
REM pictures come out of the same run and cannot drift apart.
"%ROOT%target\release\raw-autotune.exe" ^
  "%ROOT%raw\raw_3rd_batch\_DSC1282.ARW" ^
  "%ROOT%raw\raw_3rd_batch\_DSC1283.ARW" ^
  "%ROOT%raw\raw_3rd_batch\_DSC1288.ARW" ^
  "%ROOT%raw\raw_3rd_batch\_DSC1289.ARW" ^
  "%ROOT%raw\raw_3rd_batch\_DSC1290.ARW" ^
  --output "%SKY%" --format jpeg --overwrite --summary "%SKY%\sky.json" >> "%LOG%" 2>&1
echo exit=%ERRORLEVEL% >> "%LOG%"
popd
echo.
echo Sky frames in raw-autotune-output\sky-check. Look at the sky; then read
echo color.highlight_reconstruction.near_white_pixels in sky.json for how much
echo of each frame had two or more channels at the sensor clip. A large value
echo means the sky is gone in the raw and no render can bring it back.
goto done

:runsweep
REM Five frames that showed the cast, plus three neighbours from the same batch
REM that did not. The controls are the point: any setting that improves the sky
REM frames by degrading the clean ones is not an improvement, and without them
REM in the table that is invisible.
set FRAMES=_DSC1282 _DSC1283 _DSC1288 _DSC1289 _DSC1290 _DSC1280 _DSC1285 _DSC1287
set SWEEP=%ROOT%raw-autotune-output\sweep
if not exist "%SWEEP%" mkdir "%SWEEP%"

echo [4/6] cargo build --release
echo ---- cargo build --release ---->> "%LOG%"
pushd "%ROOT%"
cargo build --release --message-format=short >> "%LOG%" 2>&1
echo exit=%ERRORLEVEL% >> "%LOG%"

REM --highlight-contrast raises where highlights land without moving middle
REM grey. 1.00 is the shipped curve and is the baseline every other column is
REM read against.
echo [5/6] rendering the --highlight-contrast sweep
echo ---- highlight-contrast sweep ---->> "%LOG%"
for %%H in (1.00 1.20 1.40) do (
  echo   --highlight-contrast %%H
  if not exist "%SWEEP%\hc%%H" mkdir "%SWEEP%\hc%%H"
  for %%F in (%FRAMES%) do (
    "%ROOT%target\release\raw-autotune.exe" "%ROOT%raw\raw_3rd_batch\%%F.ARW" ^
      --output "%SWEEP%\hc%%H" --format jpeg --overwrite --no-summary ^
      --highlight-contrast %%H >> "%LOG%" 2>&1
  )
)

REM --local-tone is the sky-and-ground-on-different-exposures lever. It is
REM memory intensive; --jobs auto already budgets for it, and this renders one
REM file at a time anyway.
echo [6/6] rendering the --local-tone sweep
echo ---- local-tone sweep ---->> "%LOG%"
for %%L in (0.35 0.70) do (
  echo   --local-tone %%L
  if not exist "%SWEEP%\lt%%L" mkdir "%SWEEP%\lt%%L"
  for %%F in (%FRAMES%) do (
    "%ROOT%target\release\raw-autotune.exe" "%ROOT%raw\raw_3rd_batch\%%F.ARW" ^
      --output "%SWEEP%\lt%%L" --format jpeg --overwrite --no-summary ^
      --local-tone %%L >> "%LOG%" 2>&1
  )
)
popd

echo.
echo ---- graded against the paired camera JPEGs ----
python "%ROOT%tools\grade_sky.py" ^
  "%SWEEP%\hc1.00" "%SWEEP%\hc1.20" "%SWEEP%\hc1.40" ^
  "%SWEEP%\lt0.35" "%SWEEP%\lt0.70" ^
  --reference "%ROOT%raw\jpeg" --json "%SWEEP%\grade.json" --per-frame
goto done

:done
echo ==== check.bat finished %DATE% %TIME% ====>> "%LOG%"
echo.
echo Done. Full output in .agent\check.log
endlocal
