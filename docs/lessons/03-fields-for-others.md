# Fields for other signers

1. Say: When other people need to sign, add a field for each of them before you send the PDF.
   Screen: open "Memo.pdf"
2. Say: Drag across the page to draw a field.
   Screen: draw 1 72 598 288 638
3. Say: Name it for the person who signs there.
   Screen: rename "Signature1" "Reviewer"
4. Say: Then draw one for the next person, and name it too.
   Screen: draw 1 324 598 540 638; rename "Signature1" "Approver"
5. Say: Save Without Signing keeps both fields empty, ready for them.
   Screen: saveas "Memo for signature.pdf"; click "Save Without Signing…"; picture "fields-for-others"
6. Say: Each person opens the file and signs their own field. Here, the reviewer has signed already.
   Screen: othersigns "Memo for signature.pdf" "Reviewer" "Memo reviewed.pdf"; open "Memo reviewed.pdf"; scroll 1 610; point "✓ Signed by ROE.RICHARD.M.1234567891"
7. Say: The approver picks their field and signs it the same way.
   Screen: click "Approver"; saveas "Memo approved.pdf"; click "Sign…"
8. Say: Both signatures check out, because each new one is added after the ones before it.
   Screen: finish; picture "two-signatures"
